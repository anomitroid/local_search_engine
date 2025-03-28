use std::fs;
use std::fs::File;
use xml::common::{TextPosition, Position};
use xml::reader::{EventReader, XmlEvent};
use std::path::Path;
use std::env;
use std::process::ExitCode;
use std::result::Result;
use std::str;
use std::io::{BufReader, BufWriter};
use std::sync::{Arc, Mutex};
use std::thread;

mod model;
use model::*;
mod server;
mod lexer;
pub mod snowball;

fn parse_entire_txt_file(file_path: &Path) -> Result<String, ()> {
    fs::read_to_string(file_path).map_err(|err| {
        eprintln!("ERROR: could not open file {file_path}: {err}", file_path = file_path.display());
    })
}

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

fn parse_entire_file_by_extension(file_path: &Path) -> Result<String, ()> {
    let extension = file_path.extension().ok_or_else(|| {
        eprintln!("ERROR: cannot detect file type of {file_path} without extension", file_path = file_path.display());
    })?.to_string_lossy();
    match extension.as_ref() {
        "xhtml" | "xml" | "html" => parse_entire_xml_file(file_path),
        "txt" | "md" => parse_entire_txt_file(file_path),
        _ => {
            eprintln!("ERROR: cannot detect file type of {file_path}: unsupported extension {extension}", file_path = file_path.display(), extension = extension);
            Err(())
        }
    }
}

fn save_model_as_json(model: &InMemoryModel, index_path: &str) -> Result<(), ()> {
    println!("Saving {index_path}...");
    let index_file = File::create(index_path).map_err(|err| {
        eprintln!("ERROR: could not create index file {index_path}: {err}", index_path = index_path, err = err);
    })?;
    serde_json::to_writer_pretty(BufWriter::new(index_file), &model).map_err(|err| {
        eprintln!("ERROR: could not write index file {index_path}: {err}", index_path = index_path, err = err);
    })?;
    Ok(())
}

fn add_folder_to_model(dir_path: &Path, model: Arc<Mutex<InMemoryModel>>, skipped: &mut usize) -> Result<(), ()> {
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
        let last_modified = file.metadata().map_err(|err| {
            eprintln!("ERROR: could not get the metadata of file {file_path}: {err}", file_path = file_path.display());
        })?.modified().map_err(|err| {
            eprintln!("ERROR: could not get the last modification date of file {file_path}: {err}", file_path = file_path.display());
        })?;

        if file_type.is_dir() {
            add_folder_to_model(&file_path, Arc::clone(&model), skipped)?;
            continue 'next_file;
        }
        let mut model = model.lock().unwrap();
        if model.requires_reindexing(&file_path, last_modified)? {
            println!("Indexing {file_path:?}...", file_path = file_path);
            let content = match parse_entire_file_by_extension(&file_path) {
                Ok(content) => content.chars().collect::<Vec<_>>(),
                Err(()) => {
                    *skipped += 1;
                    continue 'next_file;
                }
            };
            model.add_document(file_path, last_modified, &content)?;
        }
        else {
            println!("Ignoring {file_path} because we have already indexed it.", file_path = file_path.display());
            *skipped += 1;
        }
    }
    Ok(())
}

fn usage(program: &str) {
    eprintln!("USAGE: {program} <subcommand> [args...]", program = program);
    eprintln!("  Subcommands:");
    eprintln!("    serve <directory> [address]         start local HTTP server with Web Interface");
}

fn entry() -> Result<(), ()> {
    let mut args = env::args();
    let program = args.next().expect("path to program is provided");
    let mut subcommand = None;
    let mut use_sqlite_mode = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sqlite" => use_sqlite_mode = true,
            _ => {
                subcommand = Some(arg);
                break
            }
        }
    }
    let subcommand = subcommand.ok_or_else(|| {
        usage(&program);
        eprintln!("ERROR: no subcommand is provided");
    })?;
    match subcommand.as_str() {
        "serve" => {
            assert!(!use_sqlite_mode);
            let dir_path = args.next().ok_or_else(|| {
                usage(&program);
                println!("ERROR: no directory path is provided for {subcommand} subcommand");
            })?;
            let index_path = "index.json";
            let address = args.next().unwrap_or("127.0.0.1:6969".to_string());
            let exists = Path::new(index_path).try_exists().map_err(|err| {
                eprintln!("ERROR: could not check the existence of file {index_path}: {err}");
            })?;
            let model: Arc<Mutex<InMemoryModel>>;
            if exists {
                let index_file = File::open(&index_path).map_err(|err| {
                    eprintln!("ERROR: could not open index file {index_path}: {err}", index_path = index_path, err = err);
                })?;
                model = Arc::new(Mutex::new(serde_json::from_reader(index_file).map_err(|err| {
                    eprintln!("ERROR: could not parse index file {index_path}: {err}", index_path = index_path, err = err);
                })?));        
            }
            else {
                model = Arc::new(Mutex::new(Default::default()));
            }
            {
                let model = Arc::clone(&model);
                thread::spawn(move || {
                    let mut skipped = 0;
                    add_folder_to_model(Path::new(&dir_path), Arc::clone(&model), &mut skipped).unwrap();
                    let model = model.lock().unwrap();
                    save_model_as_json(&model, index_path).unwrap();
                });
            }
            server::start(&address, Arc::clone(&model))
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