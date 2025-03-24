use std::path::PathBuf;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use std::result::Result;

pub type TermFreq = HashMap<String, usize>;
pub type DocFreq = HashMap<String, usize>;
pub type TermFreqPerDoc = HashMap<PathBuf, (usize, TermFreq)>;

#[derive(Default, Deserialize, Serialize)]
pub struct InMemoryModel {
    pub tfpd: TermFreqPerDoc,
    pub df: DocFreq
}

impl InMemoryModel {
    pub fn search_query(&self, query: &[char]) -> Result<Vec<(PathBuf, f32)>, ()> {
        let mut result = Vec::new();
        let tokens = Lexer::new(&query).collect::<Vec<_>>();
        for (path, (n, tf_table)) in &self.tfpd {
            let mut rank = 0f32;
            for token in &tokens {
                rank += compute_tf(&token, *n, &tf_table) * compute_idf(&token, self.tfpd.len(), &self.df);
            }
            result.push((path.clone(), rank));
        }
        result.sort_by(|(_, rank1), (_, rank2)| rank1.partial_cmp(rank2).unwrap().reverse());
        Ok(result)
    }

    pub fn add_document(&mut self, file_path: PathBuf, content: &[char]) -> Result<(), ()> {
        let mut tf = TermFreq::new();
        let mut n = 0;
        for term in Lexer::new(&content) {
            if let Some(freq) = tf.get_mut(&term) {
                *freq += 1;
            } else {
                tf.insert(term, 1);
            }
            n += 1;
        }
        for t in tf.keys() {
            if let Some(freq) = self.df.get_mut(t) {
                *freq += 1;
            } else {
                self.df.insert(t.to_string(), 1);
            }
        }
        self.tfpd.insert(file_path, (n, tf));
        Ok(())
    }
}

pub fn compute_tf(t: &str, n: usize, d: &TermFreq) -> f32 {
    let n = n as f32;
    let m = d.get(t).cloned().unwrap_or(0) as f32;
    m / n
}

pub fn compute_idf(t: &str, n: usize, df: &DocFreq) -> f32 {
    let n = n as f32;
    let m = df.get(t).cloned().unwrap_or(1) as f32;
    (n / m).log10()
}

pub struct Lexer<'a> {
    content: &'a [char],
}

impl<'a> Lexer<'a> {
    pub fn new(content: &'a [char]) -> Self {
        Self { content }
    }

    fn trim_left(&mut self) {
        while self.content.len() > 0 && self.content[0].is_whitespace() {
            self.content = &self.content[1..];
        }
    }

    fn chop(&mut self, n: usize) -> &'a [char] {
        let token = &self.content[..n];
        self.content = &self.content[n..];
        return token;
    }

    fn chop_while<P>(&mut self, mut predicate: P) -> &'a [char] where P: FnMut(&char) -> bool {
        let mut n = 0;
        while n < self.content.len() && predicate(&self.content[n]) {
            n += 1;
        }
        self.chop(n)
    }

    pub fn next_token(&mut self) -> Option<String> {
        self.trim_left();
        if self.content.len() == 0 {
            return None;
        }
        if self.content[0].is_numeric() {
            return Some(self.chop_while(|c| c.is_numeric()).iter().collect());
        }
        if self.content[0].is_alphabetic() {
            return Some(self.chop_while(|c| c.is_alphabetic()).iter().map(|x| x.to_ascii_uppercase()).collect());
        }
        return Some(self.chop(1).iter().collect());
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_token()
    }
}