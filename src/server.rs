use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use std::fs::File;
use std::{io, str};

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

fn serve_static_file(request: Request, file_path: &str, content_type: &str) -> io::Result<()> {
    let content_type_header = Header::from_bytes("Content-Type", content_type).expect("header is fine");
    let file = match File::open(file_path) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("ERROR: could not open static file {file_path}: {err}", file_path = file_path, err = err);
            if err.kind() == io::ErrorKind::NotFound {
                return serve_404(request)
            } 
            return serve_500(request)
        }
    };
    let response = Response::from_file(file).with_header(content_type_header);
    request.respond(response)
}

fn serve_api_search(model: &InMemoryModel, mut request: Request) -> io::Result<()> {
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

fn serve_request(model: &InMemoryModel, request: Request) -> io::Result<()> {
    println!("INFO: Received request! method: {:?}, url: {:?}", request.method(), request.url());
    match (request.method(), request.url()) {
        (Method::Post, "/api/search") => {
            return serve_api_search(model, request)
        },
        (Method::Get, "/index.js") => {
            return serve_static_file(request, "index.js", "text/javascript; charset=utf-8")
        }
        (Method::Get, "/") | (Method::Get, "/index.html") => {
            return serve_static_file(request, "index.html", "text/html; charset=utf-8")
        }
        _ => {
            return serve_404(request)
        }
    }
} 

pub fn start(address: &str, model: &InMemoryModel) -> Result<(), ()> {
    let server = Server::http(&address).map_err(|err| {
        eprintln!("ERROR: could not start HTTP server at {address}: {err}", address = address, err = err);
    })?;
    println!("INFO: HTTP server is running at http://{address}/", address = address);
    for request in server.incoming_requests() {
        serve_request(model, request).ok();
    }
    eprintln!("ERROR: HTTP server stopped unexpectedly");
    Err(())
}