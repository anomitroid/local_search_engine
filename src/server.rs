use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use std::{io, str};
use std::sync::{Arc, Mutex};

use super::model::*;

fn serve_404(request: Request) -> io::Result<()> {
    request.respond(Response::from_string("404").with_status_code(StatusCode(404)))
}

fn serve_500(request: Request) -> io::Result<()> {
    request.respond(Response::from_string("500").with_status_code(StatusCode(500)))
}

fn serve_400(request: Request, message: &str) -> io::Result<()> {
    request.respond(Response::from_string(format!("400: {message}")).with_status_code(StatusCode(400)))
}

fn serve_bytes(request: Request, bytes: &[u8], content_type: &str) -> io::Result<()> {
    let content_type_header = Header::from_bytes("Content-Type", content_type).expect("header is fine");
    request.respond(Response::from_data(bytes).with_header(content_type_header))
}

fn serve_api_search(model: Arc<Mutex<Box<dyn Model + Send>>>, mut request: Request) -> io::Result<()> {
    let mut buf = Vec::new();
    if let Err(err) = request.as_reader().read_to_end(&mut buf) {
        eprintln!("ERROR: could not read search request body: {err}", err = err);
        return serve_500(request)
    }
    let body = match str::from_utf8(&buf) {
        Ok(body) => body.chars().collect::<Vec<_>>(),
        Err(err) => {
            eprintln!("ERROR: could not parse search request body as UTF-8: {err}", err = err);
            return serve_400(request, "could not parse search request body as UTF-8")
        }
    };
    let model = model.lock().unwrap();
    let result = match model.search_query(&body) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("ERROR: search query failed: {err:?}", err = err);
            return serve_500(request);
        }
    };
    let json = match serde_json::to_string(&result.iter().take(20).collect::<Vec<_>>()) {
        Ok(json) => json,
        Err(err) => {
            eprintln!("ERROR: could not serialize search result as JSON: {err}", err = err);
            return serve_500(request)
        }
    };
    let content_type_header = Header::from_bytes("Content-Type", "application/json; charset=utf-8").expect("header is fine");
    let response = Response::from_string(json).with_header(content_type_header);
    return request.respond(response)
}

fn serve_api_stats(model: Arc<Mutex<Box<dyn Model + Send>>>, request: Request) -> io::Result<()> {
    use serde::Serialize;
    #[derive(Serialize)]
    struct Stats {
        docs_count: usize,
        terms_count: usize,
    }
    let mut stats = Stats {
        docs_count: 0,
        terms_count: 0,
    };

    {
        let model_guard = model.lock().unwrap();
        // Try to downcast to InMemoryModel first.
        if let Some(inmem) = model_guard.as_any().downcast_ref::<InMemoryModel>() {
            stats.docs_count = inmem.docs.len();
            stats.terms_count = inmem.df.len();
        } 
        // Otherwise assume it’s a SqliteModel.
        else if let Some(sqlite_model) = model_guard.as_any().downcast_ref::<SqliteModel>() {
            // Count documents.
            let docs_count: i64 = {
                let query = "SELECT COUNT(*) as count FROM Documents";
                let mut stmt = sqlite_model.connection.prepare(query)
                    .map_err(|err| {
                        eprintln!("ERROR: Could not prepare query {}: {}", query, err);
                        std::io::Error::new(std::io::ErrorKind::Other, "prepare failed")
                    })?;
                let count = match stmt.next().map_err(|err| {
                        eprintln!("ERROR: Could not execute query {}: {}", query, err);
                        std::io::Error::new(std::io::ErrorKind::Other, "query execution failed")
                })? {
                    sqlite::State::Row => stmt.read::<i64, _>("count")
                        .map_err(|err| {
                            eprintln!("ERROR: Could not read count from query {}: {}", query, err);
                            std::io::Error::new(std::io::ErrorKind::Other, "read failed")
                        })?,
                    _ => {
                        eprintln!("ERROR: No rows returned from query {}", query);
                        0
                    }
                };
                count
            };

            // Count terms.
            let terms_count: i64 = {
                let query = "SELECT COUNT(*) as count FROM DocFreq";
                let mut stmt = sqlite_model.connection.prepare(query)
                    .map_err(|err| {
                        eprintln!("ERROR: Could not prepare query {}: {}", query, err);
                        std::io::Error::new(std::io::ErrorKind::Other, "prepare failed")
                    })?;
                let count = match stmt.next().map_err(|err| {
                        eprintln!("ERROR: Could not execute query {}: {}", query, err);
                        std::io::Error::new(std::io::ErrorKind::Other, "query execution failed")
                })? {
                    sqlite::State::Row => stmt.read::<i64, _>("count")
                        .map_err(|err| {
                            eprintln!("ERROR: Could not read count from query {}: {}", query, err);
                            std::io::Error::new(std::io::ErrorKind::Other, "read failed")
                        })?,
                    _ => {
                        eprintln!("ERROR: No rows returned from query {}", query);
                        0
                    }
                };
                count
            };

            stats.docs_count = docs_count as usize;
            stats.terms_count = terms_count as usize;
        } else {
            eprintln!("ERROR: Unknown model type for stats");
        }
    }

    let json = serde_json::to_string(&stats).map_err(|err| {
        eprintln!("ERROR: Could not convert stats results to JSON: {}", err);
        std::io::Error::new(std::io::ErrorKind::Other, "JSON conversion failed")
    })?;

    let content_type_header = Header::from_bytes("Content-Type", "application/json")
        .expect("header is fine");
    request.respond(Response::from_string(json).with_header(content_type_header))
}

fn serve_request(model: Arc<Mutex<Box<dyn Model + Send>>>, request: Request) -> io::Result<()> {
    println!("INFO: Received request! method: {:?}, url: {:?}", request.method(), request.url());
    match (request.method(), request.url()) {
        (Method::Post, "/api/search") => {
            return serve_api_search(model, request)
        },
        (Method::Get, "/api/stats") => {
            return serve_api_stats(model, request)
        },
        (Method::Get, "/index.js") => {
            return serve_bytes(request, include_bytes!("index.js"), "text/javascript; charset=utf-8")
        }
        (Method::Get, "/") | (Method::Get, "/index.html") => {
            return serve_bytes(request, include_bytes!("index.html"), "text/html; charset=utf-8")
        }
        _ => {
            return serve_404(request)
        }
    }
} 

pub fn start(address: &str, model: Arc<Mutex<Box<dyn Model + Send>>>) -> Result<(), ()> {
    let server = Server::http(&address).map_err(|err| {
        eprintln!("ERROR: could not start HTTP server at {address}: {err}", address = address, err = err);
    })?;
    println!("INFO: HTTP server is running at http://{address}/", address = address);
    for request in server.incoming_requests() {
        serve_request(Arc::clone(&model), request).map_err(|err| {
            eprintln!("ERROR: could not serve the response: {err}");
        }).ok();
    }
    eprintln!("ERROR: HTTP server stopped unexpectedly");
    Err(())
}