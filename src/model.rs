use std::path::{PathBuf, Path};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

pub type TermFreq = HashMap<String, usize>;
pub type DocFreq = HashMap<String, usize>;
pub type TermFreqPerDoc = HashMap<PathBuf, TermFreq>;

#[allow(dead_code)]
#[derive(Default, Deserialize, Serialize)]
pub struct Model {
    pub tfpd: TermFreqPerDoc,
    pub df: DocFreq
}

pub fn tf(t: &str, d: &TermFreq) -> f32 {
    d.get(t).cloned().unwrap_or(0) as f32 / d.iter().map(|(_, f)| *f).sum::<usize>() as f32
}

pub fn idf(t: &str, d: &TermFreqPerDoc) -> f32 {
    let n = d.len() as f32;
    let m = d.values().filter(|tf| tf.contains_key(t)).count().max(1) as f32;
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

pub fn search_query<'a>(model: &'a Model, query: &'a [char]) -> Vec<(&'a Path, f32)> {
    let mut result = Vec::<(&Path, f32)>::new();
    let tokens = Lexer::new(&query).collect::<Vec<_>>();
    for (path, tf_table) in &model.tfpd {
        let mut rank = 0f32;
        for token in &tokens {
            rank += tf(&token, &tf_table) * idf(&token, &model.tfpd);
        }
        result.push((path, rank));
    }
    result.sort_by(|(_, rank1), (_, rank2)| rank1.partial_cmp(rank2).unwrap().reverse());
    result
}