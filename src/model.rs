use std::path::{Path, PathBuf};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use std::result::Result;

use super::lexer::Lexer;

pub trait Model {
    fn add_document(&mut self, path: PathBuf, content: &[char]) -> Result<(), ()>;
    fn search_query(&self, query: &[char]) -> Result<Vec<(PathBuf, f32)>, ()>;
}

pub struct SqliteModel {
    connection: sqlite::Connection
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
            eprintln!("ERROR: could not open sqlite database {path}: {err}", path = path.display());
        })?;
        let this = Self { connection };
        this.execute("
            CREATE TABLE IF NOT EXISTS Documents (
                id INTEGER NOT NULL PRIMARY KEY,
                path TEXT,
                term_count INTEGER,
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
}

impl Model for SqliteModel {
    fn add_document(&mut self, path: PathBuf, content: &[char]) -> Result<(), ()> {
        let terms = Lexer::new(content).collect::<Vec<_>>();
        let doc_id = {
            let query = "INSERT INTO Documents (path, term_count) VALUES (:path, :count)";
            let log_err = |err| {
                eprintln!("ERROR: Could not execute query {query}: {err}");
            };
            let mut stmt = self.connection.prepare(query).map_err(log_err)?;
            stmt.bind_iter::<_, (_, sqlite::Value)>([
                (":path", path.display().to_string().as_str().into()),
                (":count", (terms.len() as i64).into())
            ]).map_err(log_err)?;
            stmt.next().map_err(log_err)?;
            unsafe {
                sqlite3_sys::sqlite3_last_insert_rowid(self.connection.as_raw())
            }
        };
        let mut tf = TermFreq::new();
        for term in Lexer::new(content) {
            if let Some(freq) = tf.get_mut(&term) {
                *freq += 1;
            }
            else {
                tf.insert(term, 1);
            }
        }
        for (term, freq) in &tf {
            {
                let query = "INSERT INTO TermFreq(doc_id, term, freq) VALUES(:doc_id, :term, :freq)";
                let log_err = |err| {
                    eprintln!("ERROR: Could not execute query {query}: {err}");
                };
                let mut stmt = self.connection.prepare(query).map_err(log_err)?;
                stmt.bind_iter::<_, (_, sqlite::Value)>([
                    (":doc_id", doc_id.into()),
                    (":term", term.as_str().into()),
                    (":freq", (*freq as i64).into())
                ]).map_err(log_err)?;
                stmt.next().map_err(log_err)?;
            }
            {
                let freq = {
                    let query = "SELECT freq from DocFreq WHERE term = :term";
                    let log_err = |err| {
                        eprintln!("ERROR: Could not execute query {query}: {err}");
                    };
                    let mut stmt = self.connection.prepare(query).map_err(log_err)?;
                    stmt.bind_iter::<_, (_, sqlite::Value)>([
                        (":term", term.as_str().into())
                    ]).map_err(log_err)?;
                    match stmt.next().map_err(log_err)? {
                        sqlite::State::Row => stmt.read::<i64, _>("freq").map_err(log_err)?,
                        sqlite::State::Done => 0
                    }
                };
                let query = "INSERT OR REPLACE INTO DocFreq(term, freq) VALUES(:term, :freq)";
                let log_err = |err| {
                    eprintln!("ERROR: Could not execute query {query}: {err}");
                };
                let mut stmt = self.connection.prepare(query).map_err(log_err)?;
                stmt.bind_iter::<_, (_, sqlite::Value)>([
                    (":term", term.as_str().into()),
                    (":freq", (freq + 1).into()),
                ]).map_err(log_err)?;
                stmt.next().map_err(log_err)?;
            }
        }
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
}

pub type TermFreq = HashMap<String, usize>;
pub type DocFreq = HashMap<String, usize>;

#[derive(Default, Deserialize, Serialize)]
struct Doc {
    tf: TermFreq,
    count: usize
}

type Docs = HashMap<PathBuf, Doc>;

#[derive(Default, Deserialize, Serialize)]
pub struct InMemoryModel {
    docs: Docs,
    df: DocFreq
}

impl Model for InMemoryModel {
    fn add_document(&mut self, file_path: PathBuf, content: &[char]) -> Result<(), ()> {
        let mut tf = TermFreq::new();
        let mut count = 0;
        for term in Lexer::new(&content) {
            if let Some(freq) = tf.get_mut(&term) {
                *freq += 1;
            } else {
                tf.insert(term, 1);
            }
            count += 1;
        }
        for t in tf.keys() {
            if let Some(freq) = self.df.get_mut(t) {
                *freq += 1;
            } else {
                self.df.insert(t.to_string(), 1);
            }
        }
        self.docs.insert(file_path, Doc {count, tf});
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
            result.push((path.clone(), rank));
        }
        result.sort_by(|(_, rank1), (_, rank2)| rank1.partial_cmp(rank2).unwrap().reverse());
        Ok(result)
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