use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::time::SystemTime;
use serde::{Deserialize, Serialize};
use std::result::Result;
use std::any::Any;
use std::cell::RefCell;

use super::lexer::Lexer;

pub trait Model: Send + Any {
    fn as_any(&self) -> &dyn Any;
    fn add_document(&mut self, file_path: PathBuf, last_modified: SystemTime, fields: HashMap<String, Vec<char>>) -> Result<(), ()>;
    fn remove_document(&mut self, file_path: &std::path::Path) -> Result<(), ()>;
    fn search_query(&self, query: &[char]) -> Result<Vec<(PathBuf, f32)>, ()>;
    fn requires_reindexing(&mut self, file_path: &Path, last_modified: SystemTime) -> Result<bool, ()>;
}

pub struct SqliteModel {
    pub connection: sqlite::Connection,
    idf_cache: RefCell<Option<HashMap<String, f32>>>,
    avgdl: RefCell<Option<HashMap<String, f32>>>,
    total_docs_cache: RefCell<Option<f32>>,
}

impl SqliteModel {
    fn execute(&self, statement: &str) -> Result<(), ()> {
        self.connection.execute(statement).map_err(|err| {
            eprintln!("ERROR: could not execute query {statement}: {err}");
        })?;
        Ok(())
    }

    pub fn begin(&self) -> Result<(), ()> {
        self.execute("BEGIN;")
    } 

    pub fn commit(&self) -> Result<(), ()> {
        self.execute("COMMIT;")
    }

    pub fn open(path: &Path) -> Result<Self, ()> {
        let connection = sqlite::open(path).map_err(|err| {
            eprintln!("ERROR: could not open sqlite database {}: {}", path.display(), err);
        })?;
        let this = Self { 
            connection,
            idf_cache: RefCell::new(None),
            avgdl: RefCell::new(None),
            total_docs_cache: RefCell::new(None),
        };
        this.execute("
            CREATE TABLE IF NOT EXISTS Documents (
                id INTEGER NOT NULL PRIMARY KEY,
                path TEXT,
                last_modified INTEGER,
                UNIQUE(path)
            );
        ")?;
        this.execute("
            CREATE TABLE IF NOT EXISTS TermFreq (
                term TEXT,
                doc_id INTEGER,
                field TEXT,
                freq INTEGER,
                UNIQUE(term, doc_id, field),
                FOREIGN KEY(doc_id) REFERENCES Documents(id)
            );
        ")?;
        this.execute("
            CREATE TABLE IF NOT EXISTS DocFreq (
                term TEXT,
                freq INTEGER,
                UNIQUE(term)
            );
        ")?;
        this.execute("
            CREATE TABLE IF NOT EXISTS DocumentField (
                doc_id INTEGER,
                field TEXT,
                field_term_count INTEGER,
                UNIQUE(doc_id, field),
                FOREIGN KEY(doc_id) REFERENCES Documents(id)
            );
        ")?;
        this.update_cache()?;
        Ok(this)
    }

    fn update_cache(&self) -> Result<(), ()> {
        let total_docs = {
            let query = "SELECT COUNT(*) as count FROM Documents";
            let mut stmt = self.connection.prepare(query).map_err(|err| {
                eprintln!("ERROR: could not prepare query {}: {}", query, err);
            })?;
            let count = match stmt.next().map_err(|err| {
                eprintln!("ERROR: could not execute query {}: {}", query, err);
            })? {
                sqlite::State::Row => stmt.read::<i64, _>("count").map_err(|err| {
                    eprintln!("ERROR: Reading count: {}", err);
                })? as f32,
                _ => 0f32
            };
            count
        };
        *self.total_docs_cache.borrow_mut() = Some(total_docs);
        let mut avg_field_length = HashMap::new();
        let query = "SELECT field, AVG(field_term_count) as avglen FROM DocumentField GROUP BY field";
        let mut stmt = self.connection.prepare(query).map_err(|err| {
            eprintln!("ERROR: could not prepare query {}: {}", query, err);
        })?;
        while let sqlite::State::Row = stmt.next().map_err(|err| {
            eprintln!("ERROR: executing query {}: {}", query, err);
        })? {
            let field: String = stmt.read("field").map_err(|err| {
                eprintln!("ERROR: reading field: {}", err);
            })?;
            let avg: f64 = stmt.read("avglen").map_err(|err| {
                eprintln!("ERROR: reading avglen: {}", err);
            })?;
            avg_field_length.insert(field, avg as f32);
        }
        *self.avgdl.borrow_mut() = Some(avg_field_length);
        let mut idf_cache = HashMap::new();
        let query = "SELECT term, freq FROM DocFreq";
        let mut stmt = self.connection.prepare(query).map_err(|err| {
            eprintln!("ERROR: could not prepare query {}: {}", query, err);
        })?;
        while let sqlite::State::Row = stmt.next().map_err(|err| {
            eprintln!("ERROR: executing query {}: {}", query, err);
        })? {
            let term: String = stmt.read("term").map_err(|err| {
                eprintln!("ERROR: reading term: {}", err);
            })?;
            let df = stmt.read::<f64, _>("freq").map_err(|err| {
                eprintln!("ERROR: reading freq: {}", err);
            })? as f32;
            let idf = if df > 0.0 { ((total_docs - df + 0.5) / (df + 0.5)).ln() } else { 0f32 };
            idf_cache.insert(term, idf);
        }
        *self.idf_cache.borrow_mut() = Some(idf_cache);
        Ok(())
    }
        
    fn execute_with_binding(&self, query: &str, bindings: &[(&str, sqlite::Value)]) -> Result<(), ()> {
        let mut stmt = self.connection.prepare(query).map_err(|err| {
            eprintln!("ERROR: could not prepare query {}: {}", query, err);
        })?;
        stmt.bind_iter(bindings.iter().cloned()).map_err(|err| {
            eprintln!("ERROR: could not bind parameters for query {}: {}", query, err);
        })?;
        stmt.next().map_err(|err| {
            eprintln!("ERROR: could not execute query {}: {}", query, err);
        })?;
        Ok(())
    }
}

impl Model for SqliteModel {
    fn as_any(&self) -> &dyn Any {
        self
    }

    
    fn add_document(&mut self, path: PathBuf, last_modified: SystemTime, fields: HashMap<String, Vec<char>>) -> Result<(), ()> {
        self.begin()?;
        self.remove_document(&path)?;
        let mut unique_terms = std::collections::HashSet::new();
        let lm_ts = last_modified.duration_since(SystemTime::UNIX_EPOCH).map_err(|_| ())?.as_secs() as i64;
        let doc_id = {
            let query = "INSERT INTO Documents (path, last_modified) VALUES (:path, :last_modified)";
            let mut stmt = self.connection.prepare(query).map_err(|err| {
                eprintln!("ERROR: Could not prepare query {}: {}", query, err);
            })?;
            let bindings = vec![
                (":path", sqlite::Value::String(path.display().to_string())),
                (":last_modified", sqlite::Value::Integer(lm_ts)),
            ];
            stmt.bind_iter(bindings.iter().cloned()).map_err(|err| {
                eprintln!("ERROR: Could not bind parameters: {}", err);
            })?;
            stmt.next().map_err(|err| {
                eprintln!("ERROR: Could not execute query {}: {}", query, err);
            })?;
            unsafe {
                sqlite3_sys::sqlite3_last_insert_rowid(self.connection.as_raw())
            }
        };
        for (field, content_chars) in fields.iter() {
            let mut tf: TermFreq = HashMap::new();
            let mut count = 0;
            for token in Lexer::new(content_chars) {
                *tf.entry(token).or_insert(0) += 1;
                count += 1;
            }
            for term in tf.keys() {
                unique_terms.insert(term.clone());
            }
            {
                let query = "INSERT INTO DocumentField(doc_id, field, field_term_count) VALUES(:doc_id, :field, :field_term_count)";
                let mut stmt = self.connection.prepare(query).map_err(|err| {
                    eprintln!("ERROR: preparing query {}: {}", query, err);
                })?;
                let bindings = vec![
                    (":doc_id", sqlite::Value::Integer(doc_id)),
                    (":field", sqlite::Value::String(field.clone())),
                    (":field_term_count", sqlite::Value::Integer(count as i64)),
                ];
                stmt.bind_iter(bindings.iter().cloned()).map_err(|err| {
                    eprintln!("ERROR: binding query {}: {}", query, err);
                })?;
                stmt.next().map_err(|err| {
                    eprintln!("ERROR: executing query {}: {}", query, err);
                })?;
            }
            for (term, freq) in tf.iter() {
                let query = "INSERT INTO TermFreq(doc_id, term, field, freq) VALUES(:doc_id, :term, :field, :freq)";
                let mut stmt = self.connection.prepare(query).map_err(|err| {
                    eprintln!("ERROR: preparing query {}: {}", query, err);
                })?;
                let bindings = vec![
                    (":doc_id", sqlite::Value::Integer(doc_id)),
                    (":term", sqlite::Value::String(term.clone())),
                    (":field", sqlite::Value::String(field.clone())),
                    (":freq", sqlite::Value::Integer(*freq as i64)),
                ];
                stmt.bind_iter(bindings.iter().cloned()).map_err(|err| {
                    eprintln!("ERROR: binding parameters for {}: {}", query, err);
                })?;
                stmt.next().map_err(|err| {
                    eprintln!("ERROR: executing query {}: {}", query, err);
                })?;
            }
        }
        for term in unique_terms {
            let current_freq = {
                let query = "SELECT freq FROM DocFreq WHERE term = :term";
                let mut stmt = self.connection.prepare(query).map_err(|err| {
                    eprintln!("ERROR: preparing query {}: {}", query, err);
                })?;
                let bindings = vec![( ":term", sqlite::Value::String(term.clone()))];
                stmt.bind_iter(bindings.iter().cloned()).map_err(|err| {
                    eprintln!("ERROR: binding term for DocFreq: {}", err);
                })?;
                match stmt.next().map_err(|err| {
                    eprintln!("ERROR: executing query {}: {}", query, err);
                })? {
                    sqlite::State::Row => stmt.read::<i64, _>("freq").map_err(|err| {
                        eprintln!("ERROR: reading freq for {}: {}", term, err);
                    })? as i64,
                    sqlite::State::Done => 0,
                }
            };
            let update_query = "INSERT OR REPLACE INTO DocFreq(term, freq) VALUES(:term, :freq)";
            let mut stmt = self.connection.prepare(update_query).map_err(|err| {
                eprintln!("ERROR: preparing query {}: {}", update_query, err);
            })?;
            let new_freq = current_freq + 1;
            let bindings = vec![
                (":term", sqlite::Value::String(term.clone())),
                (":freq", sqlite::Value::Integer(new_freq)),
            ];
            stmt.bind_iter(bindings.iter().cloned()).map_err(|err| {
                eprintln!("ERROR: binding update DocFreq: {}", err);
            })?;
            stmt.next().map_err(|err| {
                eprintln!("ERROR: executing update DocFreq: {}", err);
            })?;
        }
        self.commit()?;
        Ok(())
    }    

    fn remove_document(&mut self, file_path: &std::path::Path) -> Result<(), ()> {
        let query = "SELECT id FROM Documents WHERE path = :path";
        let mut stmt = self.connection.prepare(query).map_err(|err| {
            eprintln!("ERROR: Could not prepare query {}: {}", query, err);
        })?;
        let bindings: Vec<(&str, sqlite::Value)> = vec![
            (":path", sqlite::Value::String(file_path.display().to_string()))
        ];
        stmt.bind_iter(bindings.iter().cloned()).map_err(|err| {
            eprintln!("ERROR: Could not bind path for document removal: {}", err);
        })?;
        let doc_id: i64 = match stmt.next().map_err(|err| {
            eprintln!("ERROR: Could not execute query {}: {}", query, err);
        })? {
            sqlite::State::Row => stmt.read("id").map_err(|err| {
                eprintln!("ERROR: Could not read document id: {}", err);
            })?,
            sqlite::State::Done => {
                return Ok(());
            }
        };
        let term_query = "SELECT term FROM TermFreq WHERE doc_id = :doc_id";
        let mut term_stmt = self.connection.prepare(term_query).map_err(|err| {
            eprintln!("ERROR: Could not prepare query {}: {}", term_query, err);
        })?;
        let term_bindings: Vec<(&str, sqlite::Value)> = vec![
            (":doc_id", sqlite::Value::Integer(doc_id))
        ];
        term_stmt.bind_iter(term_bindings.iter().cloned()).map_err(|err| {
            eprintln!("ERROR: Could not bind doc_id for term lookup: {}", err);
        })?;
        while let sqlite::State::Row = term_stmt.next().map_err(|err| {
            eprintln!("ERROR: Could not execute query {}: {}", term_query, err);
        })? {
            let term: String = term_stmt.read("term").map_err(|err| {
                eprintln!("ERROR: Could not read term from TermFreq: {}", err);
            })?;
            let update_query = "UPDATE DocFreq SET freq = freq - 1 WHERE term = :term";
            self.execute_with_binding(
                update_query, 
                &[
                    (":term", sqlite::Value::String(term.clone()))
                ]
            )?;
        }
        let delete_termfreq = "DELETE FROM TermFreq WHERE doc_id = :doc_id";
        {
            let mut stmt = self.connection.prepare(delete_termfreq).map_err(|err| {
                eprintln!("ERROR: Could not prepare query {}: {}", delete_termfreq, err);
            })?;
            let del_bindings: Vec<(&str, sqlite::Value)> = vec![
                (":doc_id", sqlite::Value::Integer(doc_id))
            ];
            stmt.bind_iter(del_bindings.iter().cloned()).map_err(|err| {
                eprintln!("ERROR: Could not bind doc_id for TermFreq deletion: {}", err);
            })?;
            stmt.next().map_err(|err| {
                eprintln!("ERROR: Could not execute query {}: {}", delete_termfreq, err);
            })?;
        }
        let delete_doc = "DELETE FROM Documents WHERE id = :doc_id";
        {
            let mut stmt = self.connection.prepare(delete_doc).map_err(|err| {
                eprintln!("ERROR: Could not prepare query {}: {}", delete_doc, err);
            })?;
            let del_doc_bindings: Vec<(&str, sqlite::Value)> = vec![
                (":doc_id", sqlite::Value::Integer(doc_id))
            ];
            stmt.bind_iter(del_doc_bindings.iter().cloned()).map_err(|err| {
                eprintln!("ERROR: Could not bind doc_id for Documents deletion: {}", err);
            })?;
            stmt.next().map_err(|err| {
                eprintln!("ERROR: Could not execute query {}: {}", delete_doc, err);
            })?;
        }
        Ok(())
    }

    fn search_query(&self, query: &[char]) -> Result<Vec<(PathBuf, f32)>, ()> {
        self.update_cache()?;
        let total_docs = self.total_docs_cache.borrow().unwrap();
        let avg_field_length = self.avgdl.borrow().as_ref().unwrap().clone();
        let idf_cache = self.idf_cache.borrow();
        let tokens = Lexer::new(query).collect::<Vec<_>>();
        if tokens.is_empty() {
            return Ok(vec![]);
        }
        const K1: f32 = 1.5;
        let param_names = tokens.iter().enumerate()
            .map(|(i, _)| format!(":token{i}"))
            .collect::<Vec<_>>();
        let placeholders = param_names.join(",");
        let sql = format!(
            "
            SELECT Documents.path AS path, DocumentField.field AS field, DocumentField.field_term_count AS field_length,
                   TermFreq.freq AS tf, DocFreq.freq AS df, TermFreq.term AS term
            FROM TermFreq
            JOIN Documents ON Documents.id = TermFreq.doc_id
            JOIN DocFreq ON TermFreq.term = DocFreq.term
            JOIN DocumentField ON DocumentField.doc_id = Documents.id AND DocumentField.field = TermFreq.field
            WHERE TermFreq.term IN ({placeholders})
            "
        );
        let mut stmt = self.connection.prepare(sql.as_str()).map_err(|err| {
            eprintln!("ERROR: Could not prepare search query: {}", err);
        })?;
        for (i, token) in tokens.iter().enumerate() {
            let param = format!(":token{i}");
            stmt.bind::<(&str, sqlite::Value)>((param.as_str(), sqlite::Value::String(token.clone())))
                .map_err(|err| {
                    eprintln!("ERROR: Could not bind parameter {}: {}", param, err);
                })?;
        }
        let mut scores: HashMap<PathBuf, f32> = HashMap::new();
        while let sqlite::State::Row = stmt.next().map_err(|err| {
            eprintln!("ERROR: executing search query: {}", err);
        })? {
            let path_str: String = stmt.read("path").map_err(|err| {
                eprintln!("ERROR: reading document path: {}", err);
            })?;
            let field: String = stmt.read("field").map_err(|err| {
                eprintln!("ERROR: reading field: {}", err);
            })?;
            let field_length: f32 = stmt.read::<f64, _>("field_length").map_err(|err| {
                eprintln!("ERROR: reading field_length: {}", err);
            })? as f32;
            let tf: f32 = stmt.read::<f64, _>("tf").map_err(|err| {
                eprintln!("ERROR: reading tf: {}", err);
            })? as f32;
            let df: f32 = stmt.read::<f64, _>("df").map_err(|err| {
                eprintln!("ERROR: reading df: {}", err);
            })? as f32;
            let term: String = stmt.read("term").map_err(|err| {
                eprintln!("ERROR: reading term: {}", err);
            })?;
            let avg_len = avg_field_length.get(&field).cloned().unwrap_or(field_length);
            let b = b_for_field(&field);
            let norm_tf = tf / (1.0 + b * ((field_length / avg_len) - 1.0));
            let weighted_tf = norm_tf * weights_for_fields(&field);
            let idf = idf_cache.as_ref().unwrap().get(&term).cloned().unwrap_or_else(|| ((total_docs - df + 0.5) / (df + 0.5)).ln());
            let tf_component = (weighted_tf * (K1 + 1.0)) / (weighted_tf + K1);
            let score_contribution = idf * tf_component;
            let doc_path = PathBuf::from(path_str);
            *scores.entry(doc_path).or_insert(0f32) += score_contribution;
        }
        let mut results = scores.into_iter().collect::<Vec<_>>();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        Ok(results)
    }
    
    fn requires_reindexing(&mut self, file_path: &Path, last_modified: SystemTime) -> Result<bool, ()> {
        let new_ts = last_modified.duration_since(SystemTime::UNIX_EPOCH).map_err(|_| ())?.as_secs() as i64;
        let query = "SELECT last_modified FROM Documents WHERE path = :path";
        let mut stmt = self.connection.prepare(query).map_err(|err| {
            eprintln!("ERROR: Could not prepare query {}: {}", query, err);
        })?;
        let bindings: Vec<(&str, sqlite::Value)> = vec![
            (":path", sqlite::Value::String(file_path.display().to_string()))
        ];
        stmt.bind_iter(bindings.iter().cloned()).map_err(|err| {
            eprintln!("ERROR: Could not bind path for requires_reindexing: {}", err);
        })?;
        match stmt.next().map_err(|err| {
            eprintln!("ERROR: Could not execute query {}: {}", query, err);
        })? {
            sqlite::State::Row => {
                let stored_ts: i64 = stmt.read("last_modified").map_err(|err| {
                    eprintln!("ERROR: Could not read last_modified: {}", err);
                })?;
                Ok(stored_ts < new_ts)
            },
            sqlite::State::Done => {
                Ok(true)
            }
        }
    }
}

pub type TermFreq = HashMap<String, usize>;
pub type DocFreq = HashMap<String, usize>;
pub type FieldData = (TermFreq, usize);

#[derive(Deserialize, Serialize)]
pub struct Doc {
    fields: HashMap<String, FieldData>,
    last_modified: SystemTime
}

type Docs = HashMap<PathBuf, Doc>;

#[derive(Default, Deserialize, Serialize)]
pub struct InMemoryModel {
    pub docs: Docs,
    pub df: DocFreq,
    #[serde(skip)]
    pub idf_cache: HashMap<String, f32>,
    #[serde(skip)]
    pub avg_field_length: HashMap<String, f32>,
}

fn weights_for_fields(field: &str) -> f32 {
    match field {
        "name" => 2.0,
        "content" => 1.0,
        "extension" => 0.5,
        _ => 1.0,
    }
}

fn b_for_field(field: &str) -> f32 {
    match field {
        "name" => 0.75,
        "content" => 0.75,
        "extension" => 0.75,
        _ => 0.75,
    }
}

impl InMemoryModel {
    fn update_cache(&mut self) {
        let total_docs = self.docs.len() as f32;
        let mut field_totals = HashMap::new();
        self.idf_cache.clear();
        for (_path, doc) in &self.docs {
            for (field, &(_, field_len)) in &doc.fields {
                let entry = field_totals.entry(field.to_string()).or_insert((0, 0));
                entry.0 += field_len;
                entry.1 += 1;
            }
        }
        self.avg_field_length.clear();
        for (field, (total_len, doc_count)) in field_totals {
            let avg = if doc_count > 0 {
                total_len as f32 / doc_count as f32
            }
            else {
                0f32
            };
            self.avg_field_length.insert(field, avg);
        }
        for (term, &doc_freq) in &self.df {
            let idf = if doc_freq > 0 {
                (total_docs / doc_freq as f32).ln()
            }
            else {
                0f32
            };
            self.idf_cache.insert(term.clone(), idf);
        } 
    }
}

impl Model for InMemoryModel {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn add_document(&mut self, file_path: PathBuf, last_modified: SystemTime, fields: HashMap<String, Vec<char>>) -> Result<(), ()> {
        self.remove_document(&file_path)?;
        let mut doc_fields = HashMap::new();
        let mut unique_terms = std::collections::HashSet::new();
        for (field, content) in fields {
            let mut tf = TermFreq::new();
            let mut count = 0;
            for token in Lexer::new(&content) {
                *tf.entry(token).or_insert(0) += 1;
                count += 1;
            }
            for term in tf.keys() {
                unique_terms.insert(term.clone());
            }
            doc_fields.insert(field, (tf, count));
        }
        for term in unique_terms {
            *self.df.entry(term).or_insert(0) += 1;
        }
        self.docs.insert(file_path, Doc {fields: doc_fields, last_modified});
        self.update_cache();
        Ok(())
    }

    fn remove_document(&mut self, file_path: &Path) -> Result<(), ()>{
        if let Some(doc) = self.docs.remove(file_path) {
            let mut seen_terms = std::collections::HashSet::new();
            for (_field, (tf, _)) in doc.fields {
                for term in tf.keys().cloned().collect::<Vec<_>>() {
                    seen_terms.insert(term);
                }
            }
            for term in seen_terms {
                if let Some(count) = self.df.get_mut(&term) {
                    *count = count.saturating_sub(1);
                }
            }
        }
        self.update_cache();
        Ok(())
    }

    fn search_query(&self, query: &[char]) -> Result<Vec<(PathBuf, f32)>, ()> {
        let tokens = Lexer::new(&query).collect::<Vec<_>>();
        if tokens.is_empty() {
            return Ok(vec![]);
        }
        let total_docs = self.docs.len() as f32;
        const K1: f32 = 1.5;
        let mut result = Vec::new();
        for (path, doc) in &self.docs {
            let mut score = 0f32;
            for token in &tokens {
                let mut aggregate_freq = 0f32;
                for (field, &(ref field_tf, field_len)) in &doc.fields {
                    let f = *field_tf.get(token).unwrap_or(&0) as f32;
                    if f == 0f32 {
                        continue;
                    }
                    let avg_field_len = self.avg_field_length.get(field).cloned().unwrap_or(field_len as f32);
                    let b = b_for_field(field);
                    let norm_tf = f / (1.0 + b * (field_len as f32 / avg_field_len - 1.0));
                    let weight = weights_for_fields(field);
                    aggregate_freq += weight * norm_tf;
                }
                if aggregate_freq == 0f32 {
                    continue;
                }
                let idf = self.idf_cache.get(token).cloned().unwrap_or_else(|| {
                    (total_docs / 1.0).ln()
                });
                let tf_component = (aggregate_freq * (K1 + 1.0)) / (aggregate_freq + K1);
                score += idf * tf_component;
            }
            if !score.is_nan() {
                result.push((path.clone(), score));
            }
        }
        result.sort_by(|(_, score1), (_, score2)| score2.partial_cmp(score1).expect(&format!("{score1} and {score2} are not comparable")));
        Ok(result)
    }

    fn requires_reindexing(&mut self, file_path: &Path, last_modified: SystemTime) -> Result<bool, ()> {
        if let Some(doc) = self.docs.get(file_path) {
            return Ok(doc.last_modified < last_modified);
        }
        return Ok(true);
    }
}

// fn compute_tf(t: &str, doc: &Doc) -> f32 {
//     let n = doc.count as f32;
//     let m = doc.tf.get(t).cloned().unwrap_or(0) as f32;
//     m / n
// }

// fn compute_idf(t: &str, n: usize, df: &DocFreq) -> f32 {
//     let n = n as f32;
//     let m = df.get(t).cloned().unwrap_or(1) as f32;
//     (n / m).log10()
// }


/*
------
TF-IDF
------

tf(qi, d) = f(qi, d) / |d|
where f(qi, d) is the number of times term qi appears in document d and |d| is the number of terms in document d.

idf(qi, D) = log(|D| / |{d ∈ D : qi ∈ d}|)
where |D| is the total number of documents in the collection and |{d ∈ D : qi ∈ d}| is the number of documents containing term qi.

tf gives the importance of term qi in document d
while idf gives the importance of term qi in the entire collection of documents D.

if a term qi has high tf in a document d, it means that the term is important in that document.
if a term qi has low idf in the entire collection of documents D, it means that the term is common in the entire collection.
so, a less common term is better for pinpointing a singular document where that term is important.

tfidf(qi, d, D) = tf(qi, d) * idf(qi, D)
where tfidf(t, d, D) is the score of term qi in document d in the collection of documents D.

them we sum the tfidf scores of all the terms in the query to get the score of the document for that query.

====================================================================================================================================

moving towards BM25 algorithm

----
BM25
----

score(d, Q) is the score of document d for query Q.
which is basically, still given by tf-idf i.e. tf * idf

score(d, Q) = ∑ IDF(qi) * f(qi, d) * (k1 + 1) / ( f(qi, d) + k1 * (1 - b + b * |D| / avgdl) )

qi: query term or query token in query Q
f(qi, d): frequency of term qi in document d (tf)
avgdl: average document length in the collection of documents D
|D|: number of documents in the collection of documents D
IDF(qi): inverse document frequency of term qi which weighs down terms which appear in many documents
k1: a tuning parameter (usually set between 1.2 or 2.0)
b: a tuning parameter (usually set between 0.75 and 0.95) which controls the effect of document length normalization

IDF(qi) = log(((|D| - n(qi) + 0.5) / (n(qi) + 0.5)) + 1)

n(qi): number of documents in the collection of documents D that contain term qi
|D|: number of documents in the collection of documents D



here, the raw frequency t(qi, d) is scaled by the factor:
f(qi, d) * (k1 + 1) / (f(qi, d) + k1 * (1 - b + b * |D| / avgdl))

when f(qi, d) is small, the scaling factor is close to 1.0.
when f(qi, d) is large, the scaling factor is close to k1 + 1 / k1 * (1 - b + b * |D| / avgdl)
as f(qi, d) increases, the numerator and denominator begin to balance each other out
this causes the contribution of that term (qi) to "saturate"
this avoids giving undue weight to very high-frequency terms and is more realistic than a raw frequency count.

the denominator contains the factor (1 - b + b * |D| / avgdl)
this is a normalization factor that adjusts the term frequency based on the length of the document.
for longer documents (|D| > avgdl) this factor increases the denominator
that reduces the term's contribution
for shorter documents (|D| < avgdl) this factor decreases the denominator
that increases the term's contribution
so the term's impact isnt unduly penalized

each term's contribution is furthur weighted by its inverse document frequency IDF(qi)
this reduces the weight of terms that occur in many documents
this is analogous to idf component of traditional tf-idf
ensuring that rare terms contribute more to the final score than ubiquitous ones

====================================================================================================================================

moving towards BM25F algorithm

-----
BM25F
-----

now we want to include multiple fields in out BM25
until now we were considering only the file content for search
now we want more, file name, file extension, and other file metadata

so, there will be a collection of fields F (consisting of individual fields f)
there will a corpus, or collectin of documents D (consisting of individual documents d)

each field f has:
1. it's own term frequency tf-f(qi, d) (frequency of term qi in document d for field f)
2. field specific length Lf (total number of terms in field f for document d)
3. average field length avg(Lf) (average number of terms in field f for all documents in the collection D)
4. a weight wf (relative importance of field f in the document d)
5. field specific normalization parameter bf (analogous to b im BM25)

for each field f, the normalised term frequency norm-tf-f(qi, d):
norm-tf-f(qi, d) = tf-f(qi, d) / (1 + bf * (Lf / avg(Lf) - 1))

BM25F then aggregates these normalised term frequencies across all fields weighted by their importance
F(qi, d) = ∑ wf * norm-tf-f(qi, d) = ∑ wf * tf-f(qi, d) / (1 + bf * (Lf / avg(Lf) - 1))

so, the final score of document d for query Q is given by:
score(d, Q) = ∑ IDF(qi) * F(qi, d) * (k1 + 1) / (F(qi, d) + k1)

where:
k1: a global tuning parameter controlling the saturation of term  frequency
F(qi, d): the aggregated normalised term frequency across all fields for term qi in document d
IDF(qi): inverse document frequency of term qi which weighs down terms which appear in many documents

IDF(qi) = log((|D| - n(qi) + 0.5) / (n(qi) + 0.5))

n(qi): number of documents in the collection of documents D that contain term qi
|D|: number of documents in the collection of documents D
*/