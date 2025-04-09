use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::time::SystemTime;
use serde::{Deserialize, Serialize};
use std::result::Result;
use std::any::Any;

use super::lexer::Lexer;

pub trait Model: Send + Any {
    fn as_any(&self) -> &dyn Any;
    fn add_document(&mut self, path: PathBuf, last_modified: SystemTime, content: &[char]) -> Result<(), ()>;
    fn remove_document(&mut self, file_path: &std::path::Path) -> Result<(), ()>;
    fn search_query(&self, query: &[char]) -> Result<Vec<(PathBuf, f32)>, ()>;
    fn requires_reindexing(&mut self, file_path: &Path, last_modified: SystemTime) -> Result<bool, ()>;
}

pub struct SqliteModel {
    pub connection: sqlite::Connection
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
        let this = Self { connection };
        this.execute("
            CREATE TABLE IF NOT EXISTS Documents (
                id INTEGER NOT NULL PRIMARY KEY,
                path TEXT,
                term_count INTEGER,
                last_modified INTEGER,
                UNIQUE(path)
            );
        ")?;
        this.execute("
            CREATE TABLE IF NOT EXISTS TermFreq (
                term TEXT,
                doc_id INTEGER,
                freq INTEGER,
                UNIQUE(term, doc_id),
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
        Ok(this)
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

    fn add_document(&mut self, path: PathBuf, last_modified: SystemTime, content: &[char]) -> Result<(), ()> {
        self.begin()?;
        self.remove_document(&path)?;
        let terms = Lexer::new(content).collect::<Vec<_>>();
        let lm_ts = last_modified.duration_since(SystemTime::UNIX_EPOCH).map_err(|_| ())?.as_secs() as i64;
        let doc_id = {
            let query = "INSERT INTO Documents (path, term_count, last_modified) VALUES (:path, :count, :last_modified)";
            let log_err = |err| {
                eprintln!("ERROR: Could not execute query {}: {}", query, err);
            };
            let mut stmt = self.connection.prepare(query).map_err(log_err)?;
            let bindings: Vec<(&str, sqlite::Value)> = vec![
                (":path", sqlite::Value::String(path.display().to_string())),
                (":count", sqlite::Value::Integer(terms.len() as i64)),
                (":last_modified", sqlite::Value::Integer(lm_ts)),
            ];
            stmt.bind_iter(bindings.iter().cloned()).map_err(log_err)?;
            stmt.next().map_err(log_err)?;
            unsafe {
                sqlite3_sys::sqlite3_last_insert_rowid(self.connection.as_raw())
            }
        };        
        let mut tf = TermFreq::new();
        for term in Lexer::new(content) {
            *tf.entry(term).or_insert(0) += 1;
        }
        for (term, freq) in &tf {
            {
                let query = "INSERT INTO TermFreq(doc_id, term, freq) VALUES(:doc_id, :term, :freq)";
                let log_err = |err| {
                    eprintln!("ERROR: Could not execute query {}: {}", query, err);
                };
                let mut stmt = self.connection.prepare(query).map_err(log_err)?;
                let bindings: Vec<(&str, sqlite::Value)> = vec![
                    (":doc_id", sqlite::Value::Integer(doc_id)),
                    (":term", sqlite::Value::String(term.as_str().to_string())),
                    (":freq", sqlite::Value::Integer(*freq as i64))
                ];
                stmt.bind_iter(bindings.iter().cloned()).map_err(log_err)?;
                stmt.next().map_err(log_err)?;
            }
            {
                let current_freq = {
                    let query = "SELECT freq FROM DocFreq WHERE term = :term";
                    let log_err = |err| {
                        eprintln!("ERROR: Could not execute query {}: {}", query, err);
                    };
                    let mut stmt = self.connection.prepare(query).map_err(log_err)?;
                    let bindings: Vec<(&str, sqlite::Value)> = vec![
                        (":term", sqlite::Value::String(term.as_str().to_string()))
                    ];
                    stmt.bind_iter(bindings.iter().cloned()).map_err(log_err)?;
                    match stmt.next().map_err(log_err)? {
                        sqlite::State::Row => stmt.read::<i64, _>("freq").map_err(log_err)?,
                        sqlite::State::Done => 0,
                    }
                };
                let update_query = "INSERT OR REPLACE INTO DocFreq(term, freq) VALUES(:term, :freq)";
                let log_err = |err| {
                    eprintln!("ERROR: Could not execute query {}: {}", update_query, err);
                };
                let mut stmt = self.connection.prepare(update_query).map_err(log_err)?;
                let bindings: Vec<(&str, sqlite::Value)> = vec![
                    (":term", sqlite::Value::String(term.as_str().to_string())),
                    (":freq", sqlite::Value::Integer(current_freq + 1))
                ];
                stmt.bind_iter(bindings.iter().cloned()).map_err(log_err)?;
                stmt.next().map_err(log_err)?;
            }
        }
        self.commit()?;
        Ok(())
    }

    fn search_query(&self, query: &[char]) -> Result<Vec<(PathBuf, f32)>, ()> {
        let tokens = Lexer::new(query).collect::<Vec<_>>();
        if tokens.is_empty() {
            return Ok(vec![]);
        }
        let mut param_names = Vec::new();
        for i in 0..tokens.len() {
            param_names.push(format!(":token{}", i));
        }
        let placeholders = param_names.join(",");
        let total_docs = {
            let documents_query = "SELECT COUNT(*) as count FROM Documents";
            let mut stmt = self.connection.prepare(documents_query).map_err(|err| {
                eprintln!("ERROR: Could not prepare total documents query {documents_query}: {err}");
            })?;
            let count = match stmt.next().map_err(|err| {
                eprintln!("ERROR: Failed to execute total documents query {documents_query}: {err}");
            })? {
                sqlite::State::Row => stmt.read::<i64, _>("count").map_err(|err| {
                    eprintln!("ERROR: Could not read total document count: {err}");
                })?,
                _ => {
                    eprintln!("ERROR: Total documents query returned no rows");
                    return Err(());
                }
            };
            count
        };
        let sql = format!(
            "
                SELECT Documents.path as path, Documents.term_count as term_count, TermFreq.freq as tf, DocFreq.freq as df
                FROM TermFreq
                JOIN Documents ON Documents.id = TermFreq.doc_id
                JOIN DocFreq ON TermFreq.term = DocFreq.term
                WHERE TermFreq.term IN ({})
            ", placeholders
        );
        let mut stmt = self.connection.prepare(sql.as_str()).map_err(|err| {
            eprintln!("ERROR: Could not prepare such query: {err}");
        })?;
        for (i, token) in tokens.iter().enumerate() {
            let param_name = format!(":token{}", i);
            stmt.bind::<(&str, sqlite::Value)>((param_name.as_str(), token.as_str().into())).map_err(|err| {
                eprintln!("ERROR: Could not bind parameter {} for token '{}': {err}", param_name, token, err = err);
            })?;
        }
        let mut scores = HashMap::new();
        while let sqlite::State::Row = stmt.next().map_err(|err| {
            eprintln!("ERROR: Error executing search query: {err}");
        })? {
            let path_str = stmt.read::<String, _>("path").map_err(|err| {
                eprintln!("ERROR: Could not read document path: {err}");
            })?;
            let term_count = stmt.read::<f64, _>("term_count").map_err(|err| {
                eprintln!("ERROR: Could not read document term_count: {err}");
            })?;
            let tf = stmt.read::<f64, _>("tf").map_err(|err| {
                eprintln!("ERROR: Could not read document term frequency: {err}");
            })?;
            let df = stmt.read::<f64, _>("df").map_err(|err| {
                eprintln!("ERROR: Could not read document frequncy: {err}");
            })?;
            let tf_ratio = (tf as f64) / (term_count as f64);
            let idf = ((total_docs as f64) / ((df as f64).max(1.0) as f64)).log10();
            let score = tf_ratio * idf;
            let path: PathBuf = PathBuf::from(path_str);
            *scores.entry(path).or_insert(0.0) += score;
        }
        let mut results = scores.into_iter().map(|(path, score)| (path, score as f32)).collect::<Vec<(_, _)>>();
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

#[derive(Deserialize, Serialize)]
pub struct Doc {
    tf: TermFreq,
    count: usize,
    last_modified: SystemTime
}

type Docs = HashMap<PathBuf, Doc>;

#[derive(Default, Deserialize, Serialize)]
pub struct InMemoryModel {
    pub docs: Docs,
    pub df: DocFreq
}

impl Model for InMemoryModel {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn remove_document(&mut self, file_path: &Path) -> Result<(), ()>{
        if let Some(doc) = self.docs.remove(file_path) {
            for t in doc.tf.keys() {
                if let Some(f) = self.df.get_mut(t) {
                    *f -= 1;
                }
            }
        }
        Ok(())
    }

    fn add_document(&mut self, file_path: PathBuf, last_modified: SystemTime, content: &[char]) -> Result<(), ()> {
        self.remove_document(&file_path)?;
        let mut tf = TermFreq::new();
        let mut count = 0;
        for t in Lexer::new(&content) {
            if let Some(f) = tf.get_mut(&t) {
                *f += 1;
            } else {
                tf.insert(t, 1);
            }
            count += 1;
        }
        for t in tf.keys() {
            if let Some(f) = self.df.get_mut(t) {
                *f += 1;
            } else {
                self.df.insert(t.to_string(), 1);
            }
        }
        self.docs.insert(file_path, Doc {count, tf, last_modified});
        Ok(())
    }

    fn search_query(&self, query: &[char]) -> Result<Vec<(PathBuf, f32)>, ()> {
        let mut result = Vec::new();
        let tokens = Lexer::new(&query).collect::<Vec<_>>();
        for (path, doc) in &self.docs {
            let mut rank = 0f32;
            for token in &tokens {
                rank += compute_tf(&token, doc) * compute_idf(&token, self.docs.len(), &self.df);
            }
            if !rank.is_nan() {
                result.push((path.clone(), rank));
            }
        }
        result.sort_by(|(_, rank1), (_, rank2)| rank1.partial_cmp(rank2).expect(&format!("{rank1} and {rank2} are not comparable")));
        result.reverse();
        Ok(result)
    }

    fn requires_reindexing(&mut self, file_path: &Path, last_modified: SystemTime) -> Result<bool, ()> {
        if let Some(doc) = self.docs.get(file_path) {
            return Ok(doc.last_modified < last_modified);
        }
        return Ok(true);
    }
}

fn compute_tf(t: &str, doc: &Doc) -> f32 {
    let n = doc.count as f32;
    let m = doc.tf.get(t).cloned().unwrap_or(0) as f32;
    m / n
}

fn compute_idf(t: &str, n: usize, df: &DocFreq) -> f32 {
    let n = n as f32;
    let m = df.get(t).cloned().unwrap_or(1) as f32;
    (n / m).log10()
}


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

score(d, Q) = ∑ IDF(qi) * f(qi, d) * (k + 1) / ( f(qi, d) + k * (1 - b + b * |D| / avgdl) )

qi: query term or query token in query Q
f(qi, d): frequency of term qi in document d (tf)
avgdl: average document length in the collection of documents D
|D|: number of documents in the collection of documents D
IDF(qi): inverse document frequency of term qi which weighs down terms which appear in many documents
k: a tuning parameter (usually set between 1.2 or 2.0)
b: a tuning parameter (usually set between 0.75 and 0.95) which controls the effect of document length normalization

IDF(qi) = log(((|D| - n(qi) + 0.5) / (n(qi) + 0.5)) + 1)

n(qi): number of documents in the collection of documents D that contain term qi
|D|: number of documents in the collection of documents D



here, the raw frequency t(qi, d) is scaled by the factor:
f(qi, d) * (k + 1) / (f(qi, d) + k * (1 - b + b * |D| / avgdl))

when f(qi, d) is small, the scaling factor is close to 1.0.
when f(qi, d) is large, the scaling factor is close to k + 1 / k * (1 - b + b * |D| / avgdl)
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

*/