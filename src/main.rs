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
use std::str;

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

    fn next_token(&mut self) -> Option<String> {
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
        for term in Lexer::new(&content) {
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
    eprintln!("    index <dir_path>                     index all XML files in the directory and save the index to index.json");
    eprintln!("    search <index_path>                  search the index file");
    eprintln!("    serve <index_path> [address]         start local HTTP server with Web Interface");
}

fn serve_static_file(request: Request, file_path: &str, content_type: &str) -> Result<(), ()> {
    let content_type_header = Header::from_bytes("Content-Type", content_type).expect("header is fine");
    let file = File::open(file_path).map_err(|err| {
        eprintln!("ERROR: could serve {file_path}: {err}", file_path = file_path, err = err);
    })?;
    let response = Response::from_file(file).with_header(content_type_header);
    request.respond(response).map_err(|err| {
        eprintln!("ERROR: could not serve static file {file_path}: {err}", file_path = file_path, err = err);
    })
}

fn serve_404(request: Request) -> Result<(), ()> {
    request.respond(Response::from_string("404").with_status_code(StatusCode(404))).map_err(|err| {
        eprintln!("ERROR: could not respond to request: {err}", err = err);
    })
}

fn tf(t: &str, d: &TermFreq) -> f32 {
    d.get(t).cloned().unwrap_or(0) as f32 / d.iter().map(|(_, f)| *f).sum::<usize>() as f32
}

fn serve_request(tf_index: &TermFreqIndex, mut request: Request) -> Result<(), ()> {
    println!("INFO: Received request! method: {:?}, url: {:?}", request.method(), request.url());
    match (request.method(), request.url()) {
        (Method::Post, "/api/search") => {
            let mut buf = Vec::new();
            request.as_reader().read_to_end(&mut buf).map_err(|err| {
                eprintln!("ERROR: could not read body of search request: {err}", err = err);
            })?;
            let body = str::from_utf8(&buf).map_err(|err| {
                eprintln!("ERROR: could not interpret body as UTF-8 string: {err}", err = err);
            })?.chars().collect::<Vec<_>>();
            let mut result = Vec::<(&Path, f32)>::new();
            for (path, tf_table) in tf_index {
                let mut total_tf = 0f32;
                for token in Lexer::new(&body) {
                    total_tf += tf(&token, &tf_table);
                }
                result.push((path, total_tf));
            }
            result.sort_by(|(_, rank1), (_, rank2)| rank1.partial_cmp(rank2).unwrap().reverse());
            for (path, rank) in result {
                println!("{path} => {rank}", path = path.display(), rank = rank);
            }
            request.respond(Response::from_string("ok")).map_err(|err| {
                eprintln!("ERROR: could not respond to search request: {err}", err = err);
            })?
        },
        (Method::Get, "/index.js") => serve_static_file(request, "index.js", "text/javascript; charset=utf-8")?,
        (Method::Get, "/") | (Method::Get, "/index.html") => serve_static_file(request, "index.html", "text/html; charset=utf-8")?,
        _ => serve_404(request)?
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
                println!("ERROR: no index file path is provided for {} subcommand", subcommand);
            })?;
            check_index(&index_path)?;
        },
        "serve" => {
            let index_path = args.next().ok_or_else(|| {
                usage(&program);
                println!("ERROR: no index file path is provided for {} subcommand", subcommand);
            })?;
            let index_file = File::open(&index_path).map_err(|err| {
                eprintln!("ERROR: could not open index file {index_path}: {err}", index_path = index_path, err = err);
            })?;
            let tf_index: TermFreqIndex = serde_json::from_reader(index_file).map_err(|err| {
                eprintln!("ERROR: could not parse index file {index_path}: {err}", index_path = index_path, err = err);
            })?;        
            let address = args.next().unwrap_or("127.0.0.1:6969".to_string());
            let server = Server::http(&address).map_err(|err| {
                eprintln!("ERROR: could not start HTTP server at {address}: {err}", address = address, err = err);
            })?;
            println!("INFO: HTTP server is running at http://{address}/", address = address);
            for request in server.incoming_requests() {
                serve_request(&tf_index, request)?;
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