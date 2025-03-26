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

mod model;
use model::*;
mod server;
mod lexer;

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

fn add_folder_to_model(dir_path: &Path, model: &mut dyn Model, skipped: &mut usize) -> Result<(), ()> {
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
            add_folder_to_model(&file_path, model, skipped)?;
            continue 'next_file;
        }
        println!("Indexing {file_path:?}...", file_path = file_path);
        let content = match parse_entire_file_by_extension(&file_path) {
            Ok(content) => content.chars().collect::<Vec<_>>(),
            Err(()) => {
                *skipped += 1;
                continue 'next_file;
            }
        };
        model.add_document(file_path, &content)?;
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
        "index" => {
            let dir_path = args.next().ok_or_else(|| {
                usage(&program);
                println!("ERROR: no directory path is provided");
            })?;
            let mut skipped = 0;
            if use_sqlite_mode {                
                let index_path = "index.db";
                if let Err(err) = fs::remove_file(index_path) {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        eprintln!("ERROR: could not delete file {index_path}: {err}");
                        return Err(())
                    }
                }
                let mut model = SqliteModel::open(Path::new(index_path))?;
                model.begin()?;
                add_folder_to_model(Path::new(&dir_path), &mut model, &mut skipped)?;
                model.commit()?;
            }
            else {
                let index_path = "index.json";
                let mut model = Default::default();
                add_folder_to_model(Path::new(&dir_path), &mut model, &mut skipped)?;
                save_model_as_json(&model, index_path)?;
            }
            println!("Skipped {skipped} files.");
            Ok(())
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
            if use_sqlite_mode {
                let model = SqliteModel::open(Path::new(&index_path))?;
                for (path, rank) in model.search_query(&prompt)?.iter().take(20) {
                    println!("{path} {rank}", path = path.display());
                }
            }
            else {
                let index_file = File::open(&index_path).map_err(|err| {
                    eprintln!("ERROR: could not open index file {index_path}: {err}");
                })?;
                let model = serde_json::from_reader::<_, InMemoryModel>(index_file).map_err(|err| {
                    eprintln!("ERROR: could not parse index file {index_path}: {err}");
                })?;
                for (path, rank) in model.search_query(&prompt)?.iter().take(20) {
                    println!("{path} {rank}", path = path.display());
                }
            }
            return Ok(());
        },
        "serve" => {
            let index_path = args.next().ok_or_else(|| {
                usage(&program);
                println!("ERROR: no index file path is provided for {} subcommand", subcommand);
            })?;
            let address = args.next().unwrap_or("127.0.0.1:6969".to_string());
            if use_sqlite_mode {
                let model = SqliteModel::open(Path::new(&index_path))?;
                return server::start(&address, &model);
            }
            else {
                let index_file = File::open(&index_path).map_err(|err| {
                    eprintln!("ERROR: could not open index file {index_path}: {err}", index_path = index_path, err = err);
                })?;
                let model: InMemoryModel = serde_json::from_reader(index_file).map_err(|err| {
                    eprintln!("ERROR: could not parse index file {index_path}: {err}", index_path = index_path, err = err);
                })?;        
                return server::start(&address, &model);    
            }
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