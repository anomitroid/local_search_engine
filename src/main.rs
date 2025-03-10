use std::{fs, io};
use std::fs::File;
use xml::reader::{EventReader, XmlEvent};
use std::path::Path;

fn read_entire_xml_file<P: AsRef<Path>>(file_path: P) -> io::Result<String> {
    let file = File::open(file_path)?;
    let er = EventReader::new(file);
    let mut content = String::new();
    for event in er.into_iter() {
        if let XmlEvent::Characters(text) = event.expect("TODO") {
            content.push_str(&text);
        }
    }
    Ok(content)
}

fn main() -> io::Result<()> {
    let dir_path = "docs.gl/gl4/";
    let dir = fs::read_dir(dir_path)?;
    for file in dir {
        let file_path = file?.path();
        let content = read_entire_xml_file(&file_path)?;
        println!("{file_path:?} => {size}", size = content.len());
    }
    // println!("{content}", content = read_entire_xml_file(file_path).expect("TODO"));
    Ok(())
}
