#[derive(Debug, Eq, PartialEq)]
pub struct Part;

#[derive(Debug, Eq, PartialEq)]
pub struct Mime {
    parts: Vec<Part>,
    seperator: String,
}
