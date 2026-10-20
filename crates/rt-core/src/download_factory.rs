use crate::download::Download;
use crate::download_list::DownloadList;
use lt_common::Object;
use lt_engine::constructor::DownloadConstructor;
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::sync::{Arc, RwLock};

pub struct DownloadFactory {
    object: Option<Object>,
    magnet: Option<String>,
    start: bool,
    session: bool,
    init_load: bool,
    print_log: bool,
    commands: Vec<String>,
}

impl Default for DownloadFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadFactory {
    pub fn new() -> Self {
        Self {
            object: None,
            magnet: None,
            start: false,
            session: false,
            init_load: false,
            print_log: true,
            commands: Vec::new(),
        }
    }

    pub fn set_start(&mut self, start: bool) {
        self.start = start;
    }

    pub fn get_start(&self) -> bool {
        self.start
    }

    pub fn set_session(&mut self, session: bool) {
        self.session = session;
    }

    pub fn get_session(&self) -> bool {
        self.session
    }

    pub fn set_init_load(&mut self, init_load: bool) {
        self.init_load = init_load;
    }

    pub fn get_init_load(&self) -> bool {
        self.init_load
    }

    pub fn set_print_log(&mut self, print_log: bool) {
        self.print_log = print_log;
    }

    pub fn print_log(&self) -> bool {
        self.print_log
    }

    pub fn commands(&mut self) -> &mut Vec<String> {
        &mut self.commands
    }

    pub fn load_object(&mut self, object: Object) {
        self.magnet = None;
        self.object = Some(object);
    }

    pub fn load_raw_data(&mut self, data: &[u8]) -> io::Result<()> {
        let object = lt_common::object::bencode::decode(data)
            .map_err(|e| Error::new(ErrorKind::InvalidData, format!("bencode error: {}", e)))?;
        self.load_object(object);
        Ok(())
    }

    pub fn load_file(&mut self, path: &str) -> io::Result<()> {
        let bytes = fs::read(path)?;
        self.load_raw_data(&bytes)
    }

    /// Loads a magnet URI. Metadata is fetched from peers (`ut_metadata`) after
    /// the download is started; until then the placeholder only carries the
    /// info hash, display name and trackers from the URI.
    pub fn load_magnet(&mut self, uri: &str) -> io::Result<()> {
        if !uri.starts_with("magnet:?") {
            return Err(Error::new(ErrorKind::InvalidInput, "not a magnet URI"));
        }
        self.object = None;
        self.magnet = Some(uri.to_string());
        Ok(())
    }

    pub fn create_download(&self) -> io::Result<Download> {
        if let Some(uri) = &self.magnet {
            let metadata = DownloadConstructor::from_magnet_uri(uri)
                .map_err(|e| Error::new(ErrorKind::InvalidData, format!("magnet error: {}", e)))?;
            return Ok(Download::from_metadata(metadata));
        }

        let object = self
            .object
            .as_ref()
            .ok_or_else(|| Error::new(ErrorKind::NotFound, "DownloadFactory: no torrent loaded"))?;

        let metadata = DownloadConstructor::from_object(object)
            .map_err(|e| Error::new(ErrorKind::InvalidData, format!("metadata error: {}", e)))?;

        let mut download = Download::from_metadata(metadata);
        download.set_bencode(object.clone());
        Ok(download)
    }

    pub fn commit(
        &mut self,
        download_list: &mut DownloadList,
    ) -> io::Result<Arc<RwLock<Download>>> {
        let download = self.create_download()?;
        let dl_arc = Arc::new(RwLock::new(download));

        if !download_list.insert(Arc::clone(&dl_arc)) {
            return Err(Error::new(
                ErrorKind::AlreadyExists,
                "Download already exists in DownloadList",
            ));
        }

        if self.start {
            download_list.start(&dl_arc);
        }

        Ok(dl_arc)
    }
}
