use std::path::PathBuf;

pub struct Gateway {
    file: PathBuf,

    repository: String,
    workflow: String,
}

impl Gateway {
    pub fn new(file: PathBuf, repository: String, workflow: String) -> Self {
        Self { file, repository, workflow }
    }
}