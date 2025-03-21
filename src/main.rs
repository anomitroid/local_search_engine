use std::fs;
use std::fs::File;
use xml::common::{TextPosition, Position};
use xml::reader::{EventReader, XmlEvent};
use std::path::Path;
use std::env;
use std::process::{exit, ExitCode};
use std::result::Result;
use std::str;
use std::io::{BufReader, BufWriter};

mod model;
use model::*;
mod server;

fn parse_entire_xml_file(file_path: &Path) -> Result<String, ()> {
    let file = File::open(file_path).map_err(|err| {
        eprintln!("ERROR: could not open file {file_path}: {err}", file_path = file_path.display(), err = err);
    })?;
    let er = EventReader::new(BufReader::new(file));
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

fn save_model_as_json(model: &Model, index_path: &str) -> Result<(), ()> {
    println!("Saving {index_path}...");
    let index_file = File::create(index_path).map_err(|err| {
        eprintln!("ERROR: could not create index file {index_path}: {err}", index_path = index_path, err = err);
    })?;
    serde_json::to_writer_pretty(BufWriter::new(index_file), &model).map_err(|err| {
        eprintln!("ERROR: could not write index file {index_path}: {err}", index_path = index_path, err = err);
    })?;
    Ok(())
}

fn add_folder_to_model(dir_path: &Path, model: &mut Model) -> Result<(), ()> {
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
            add_folder_to_model(&file_path, model)?;
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
        for t in tf.keys() {
            if let Some(freq) = model.df.get_mut(t) {
                *freq += 1;
            } else {
                model.df.insert(t.to_string(), 1);
            }
        }
        model.tfpd.insert(file_path, tf);
    }
    Ok(())
}

fn usage(program: &str) {
    eprintln!("USAGE: {program} <subcommand> [args...]", program = program);
    eprintln!("  Subcommands:");
    eprintln!("    index <dir_path>                     index all XML files in the directory and save the index to index.json");
    eprintln!("    search <index_path> <query>          search for a query within the index file");
    eprintln!("    serve <index_path> [address]         start local HTTP server with Web Interface");
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
            let mut model = Default::default();
            add_folder_to_model(Path::new(&dir_path), &mut model)?;
            return save_model_as_json(&model, "index.json");
        },
        "search" => {
            let index_path = args.next().ok_or_else(|| {
                usage(&program);
                println!("ERROR: no index file path is provided for {} subcommand", subcommand);
            })?;
            let prompt = args.next().ok_or_else(|| {
                usage(&program);
                println!("ERROR: no query is provided for {} subcommand", subcommand);
            })?.chars().collect::<Vec<_>>();
            let index_file = File::open(&index_path).map_err(|err| {
                eprintln!("ERROR: could not open index file {index_path}: {err}", index_path = index_path, err = err);
            })?;
            let model: Model = serde_json::from_reader(index_file).map_err(|err| {
                eprintln!("ERROR: could not parse index file {index_path}: {err}", index_path = index_path, err = err);
            })?;
            for (path, rank) in search_query(&model, &prompt).iter().take(20) {
                println!("{path} {rank}", rank = rank, path = path.display());
            }
            return Ok(());
        },
        "serve" => {
            let index_path = args.next().ok_or_else(|| {
                usage(&program);
                println!("ERROR: no index file path is provided for {} subcommand", subcommand);
            })?;
            let index_file = File::open(&index_path).map_err(|err| {
                eprintln!("ERROR: could not open index file {index_path}: {err}", index_path = index_path, err = err);
            })?;
            let model: Model = serde_json::from_reader(index_file).map_err(|err| {
                eprintln!("ERROR: could not parse index file {index_path}: {err}", index_path = index_path, err = err);
            })?;        
            let address = args.next().unwrap_or("127.0.0.1:6969".to_string());
            return server::start(&address, &model);
        },
        _ => {
            usage(&program);
            println!("ERROR: unknown subcommand {subcommand}");
            return Err(());
        }
    }
}

fn main() -> ExitCode {
    match entry() {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}