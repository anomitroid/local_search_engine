use std::fs;
use std::fs::File;
use xml::common::{TextPosition, Position};
use xml::reader::{EventReader, XmlEvent};
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::env;
use std::process::{exit, ExitCode};
use std::result::Result;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

struct Lexer<'a> {
    content: &'a [char],
}

impl<'a> Lexer<'a> {
    fn new(content: &'a [char]) -> Self {
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

    fn next_token(&mut self) -> Option<&'a [char]> {
        self.trim_left();
        if self.content.len() == 0 {
            return None;
        }
        if self.content[0].is_numeric() {
            return Some(self.chop_while(|c| c.is_numeric()));
        }
        if self.content[0].is_alphabetic() {
            return Some(self.chop_while(|c| c.is_alphabetic()));
        }
        return Some(self.chop(1));
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = &'a [char];

    fn next(&mut self) -> Option<Self::Item> {
        self.next_token()
    }
}

fn parse_entire_xml_file(file_path: &Path) -> Result<String, ()> {
    let file = File::open(file_path).map_err(|err| {
        eprintln!("ERROR: could not open file {file_path}: {err}", file_path = file_path.display(), err = err);
    })?;
    let er = EventReader::new(file);
    let mut content = String::new();
    for event in er.into_iter() {
        let event = event.map_err(|err| {
            let TextPosition {row, column} = err.position();
            let msg = err.msg();
            eprintln!("{file_path}:{row}:{column}: ERROR: {msg}", file_path = file_path.display(), row = row, column = column, msg = msg);
        })?;
        if let XmlEvent::Characters(text) = event {
            content.push_str(&text);
            content.push_str(" ");
        }
    }
    Ok(content)
}

type TermFreq = HashMap<String, usize>;
type TermFreqIndex = HashMap<PathBuf, TermFreq>;

fn check_index(index_path: &str) -> Result<(), ()> {
    println!("Reading {} index file...", index_path);
    let index_file = File::open(index_path).map_err(|err| {
        eprintln!("ERROR: could not open index file {index_path}: {err}", index_path = index_path, err = err);
    })?;
    let tf_index: TermFreqIndex = serde_json::from_reader(index_file).map_err(|err| {
        eprintln!("ERROR: could not parse index file {index_path}: {err}", index_path = index_path, err = err);
    })?;
    println!("{} index file contains {} files", index_path, tf_index.len());
    Ok(())
}

fn save_tf_index(tf_index: &TermFreqIndex, index_path: &str) -> Result<(), ()> {
    println!("Saving {index_path}...");
    let index_file = File::create(index_path).map_err(|err| {
        eprintln!("ERROR: could not create index file {index_path}: {err}", index_path = index_path, err = err);
    })?;
    serde_json::to_writer_pretty(index_file, &tf_index).map_err(|err| {
        eprintln!("ERROR: could not write index file {index_path}: {err}", index_path = index_path, err = err);
    })?;
    Ok(())
}

fn tf_index_of_dir(dir_path: &Path, tf_index: &mut TermFreqIndex) -> Result<(), ()> {
    let dir = fs::read_dir(dir_path).map_err(|err| {
        eprintln!("ERROR: could not read directory {dir_path}: {err}", dir_path = dir_path.display(), err = err);
    })?;
    'next_file: for file in dir {
        let file = file.map_err(|err| {
            eprintln!("ERROR: could not read next file in directory {dir_path} during indexing: {err}", dir_path = dir_path.display(), err = err);
        })?;
        let file_path = file.path();
        let file_type = file.file_type().map_err(|err| {
            eprintln!("ERROR: could not get file type of {file_path}: {err}", file_path = file_path.display(), err = err);
        })?;
        if file_type.is_dir() {
            tf_index_of_dir(&file_path, tf_index)?;
            continue 'next_file;
        }
        println!("Indexing {file_path:?}...", file_path = file_path);
        let content = match parse_entire_xml_file(&file_path) {
            Ok(content) => content.chars().collect::<Vec<_>>(),
            Err(()) => continue 'next_file,
        };
        let mut tf = TermFreq::new();
        for token in Lexer::new(&content) {
            let term = token.iter().map(|x| x.to_ascii_uppercase()).collect::<String>();
            if let Some(freq) = tf.get_mut(&term) {
                *freq += 1;
            } else {
                tf.insert(term, 1);
            }
        }
        tf_index.insert(file_path, tf);
    }
    Ok(())
}

fn usage(program: &str) {
    eprintln!("USAGE: {program} <subcommand> [args...]", program = program);
    eprintln!("  Subcommands:");
    eprintln!("    index <dir_path>         index all XML files in the directory and save the index to index.json");
    eprintln!("    search <index_path>      search the index file");
    eprintln!("    serve [address]          start local HTTP server with Web Interface");
}

fn serve_request(request: Request) -> Result<(), ()> {
    println!("INFO: Received request! method: {:?}, url: {:?}", request.method(), request.url());
    match (request.method(), request.url()) {
        (Method::Get, "/index.js") => {
            let content_type_text_javascript = Header::from_bytes("Content-Type", "text/javascript; charset=utf-8").expect("header is fine");
            let index_js_path = "index.js";
            let index_js_file = File::open(index_js_path).map_err(|err| {
                eprintln!("ERROR: could not open {index_js_path}: {err}", index_js_path = index_js_path, err = err);
            })?;
            let response = Response::from_file(index_js_file).with_header(content_type_text_javascript);
            request.respond(response).map_err(|err| {
                eprintln!("ERROR: could not respond to request: {err}", err = err);
            })?;        
        }
        (Method::Get, "/") | (Method::Get, "/index.html") => {
            let content_type_text_html = Header::from_bytes("Content-Type", "text/html; charset=utf-8").expect("header is fine");
            let index_html_path = "index.html";
            let index_html_file = File::open(index_html_path).map_err(|err| {
                eprintln!("ERROR: could not open {index_html_path}: {err}", index_html_path = index_html_path, err = err);
            })?;
            let response = Response::from_file(index_html_file).with_header(content_type_text_html);
            request.respond(response).map_err(|err| {
                eprintln!("ERROR: could not respond to request: {err}", err = err);
            })?;        
        },
        _ => {
            request.respond(Response::from_string("404").with_status_code(StatusCode(404))).map_err(|err| {
                eprintln!("ERROR: could not respond to request: {err}", err = err);
            })?;
        }
    }
    Ok(())
} 

fn entry() -> Result<(), ()> {
    let mut args = env::args();
    let program = args.next().expect("path to program is provided");
    let subcommand = args.next().ok_or_else(|| {
        println!("ERROR: no subcommand is provided");
        exit(1);
    })?;
    match subcommand.as_str() {
        "index" => {
            let dir_path = args.next().ok_or_else(|| {
                usage(&program);
                println!("ERROR: no directory path is provided");
            })?;
            let mut tf_index = TermFreqIndex::new();
            tf_index_of_dir(Path::new(&dir_path), &mut tf_index)?;
            save_tf_index(&tf_index, "index.json")?;
        },
        "search" => {
            let index_path = args.next().ok_or_else(|| {
                usage(&program);
                println!("ERROR: no index file path is provided");
            })?;
            check_index(&index_path)?;
        },
        "serve" => {
            let address = args.next().unwrap_or("127.0.0.1:6969".to_string());
            let server = Server::http(&address).map_err(|err| {
                eprintln!("ERROR: could not start HTTP server at {address}: {err}", address = address, err = err);
            })?;
            println!("INFO: HTTP server is running at http://{address}/", address = address);
            for request in server.incoming_requests() {
                serve_request(request);
            }
        },
        _ => {
            usage(&program);
            println!("ERROR: unknown subcommand {subcommand}");
            return Err(())
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match entry() {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}