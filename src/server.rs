use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use std::fs::File;
use std::str;
use super::model::*;

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

fn serve_api_search(tf_index: &TermFreqIndex, mut request: Request) -> Result<(), ()> {
    let mut buf = Vec::new();
    request.as_reader().read_to_end(&mut buf).map_err(|err| {
        eprintln!("ERROR: could not read body of search request: {err}", err = err);
    })?;
    let body = str::from_utf8(&buf).map_err(|err| {
        eprintln!("ERROR: could not interpret body as UTF-8 string: {err}", err = err);
    })?.chars().collect::<Vec<_>>();
    let result = search_query(tf_index, &body);
    let json = serde_json::to_string(&result.iter().take(20).collect::<Vec<_>>()).map_err(|err| {
        eprintln!("ERROR: could not convert search results to JSON: {err}", err = err);
    })?;
    let content_type_header = Header::from_bytes("Content-Type", "application/json; charset=utf-8").expect("header is fine");
    let response = Response::from_string(json).with_header(content_type_header);
    return request.respond(response).map_err(|err| {
        eprintln!("ERROR: could not respond to search request: {err}", err = err);
    })
}

fn serve_request(tf_index: &TermFreqIndex, request: Request) -> Result<(), ()> {
    println!("INFO: Received request! method: {:?}, url: {:?}", request.method(), request.url());
    match (request.method(), request.url()) {
        (Method::Post, "/api/search") => {
            return serve_api_search(tf_index, request)
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

pub fn start(address: &str, tf_index: &TermFreqIndex) -> Result<(), ()> {
    let server = Server::http(&address).map_err(|err| {
        eprintln!("ERROR: could not start HTTP server at {address}: {err}", address = address, err = err);
    })?;
    println!("INFO: HTTP server is running at http://{address}/", address = address);
    for request in server.incoming_requests() {
        serve_request(&tf_index, request).ok();
    }
    eprintln!("ERROR: HTTP server stopped unexpectedly");
    Err(())
}