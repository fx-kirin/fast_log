use std::cell::RefCell;
use std::fs::{DirBuilder, DirEntry, File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write, Error};

use chrono::{Local, NaiveDateTime};
use zip::write::FileOptions;

use crate::appender::{Command, FastLogRecord, LogAppender};
use crate::consts::LogSize;
use std::ops::Sub;
use std::time::Duration;
use may::go;
use may::sync::mpsc::{Receiver, Sender};
use zip::result::ZipResult;
use crate::error::LogError;

/// .zip or .lz4 or any one packer
pub trait Packer: Send {
    fn pack_name(&self) -> &'static str;
    //return bool: remove_log_file
    fn do_pack(&self, log_file: File, log_file_path: &str) -> Result<bool, LogError>;
    /// default 0 is not retry pack. if retry > 0 ,it will trying rePack
    fn retry(&self) -> i32 { return 0; }
}

/// split log file allow compress log
pub struct FileSplitAppender {
    cell: RefCell<FileSplitAppenderData>,
}

///log data pack
pub struct LogPack {
    pub dir: String,
    pub rolling: RollingType,
    pub new_log_name: String,
}

///rolling keep type
#[derive(Copy, Clone, Debug)]
pub enum RollingType {
    All,
    KeepTime(Duration),
    KeepNum(i64),
}

impl RollingType {
    fn read_paths(&self, dir: &str) -> Vec<DirEntry> {
        let paths = std::fs::read_dir(dir);
        if let Ok(paths) = paths {
            let mut paths_vec = vec![];
            for path in paths {
                match path {
                    Ok(path) => {
                        if let Some(v) = path.file_name().to_str() {
                            //filter temp.log and not start with temp
                            if v.ends_with("temp.log") || !v.starts_with("temp") {
                                continue;
                            }
                        }
                        paths_vec.push(path);
                    }
                    _ => {}
                }
            }
            paths_vec.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
            return paths_vec;
        }
        return vec![];
    }

    pub fn do_rolling(&self, dir: &str) {
        match self {
            RollingType::KeepNum(n) => {
                let paths_vec = self.read_paths(dir);
                for index in 0..paths_vec.len() {
                    if index >= *n as usize {
                        let item = &paths_vec[index];
                        std::fs::remove_file(item.path());
                    }
                }
            }
            RollingType::KeepTime(t) => {
                let paths_vec = self.read_paths(dir);
                let duration = chrono::Duration::from_std(t.clone());
                if duration.is_err() {
                    return;
                }
                let duration = duration.unwrap();
                let now = Local::now().naive_local();
                for index in 0..paths_vec.len() {
                    let item = &paths_vec[index];
                    let file_name = item.file_name();
                    let name = file_name.to_str().unwrap_or("").to_string();
                    if let Some(time) = self.file_name_parse_time(&name) {
                        if now.sub(time) > duration {
                            std::fs::remove_file(item.path());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn file_name_parse_time(&self, name: &str) -> Option<NaiveDateTime> {
        if name.starts_with("temp") {
            let mut time_str = name.replace("temp", "");
            if let Some(v) = time_str.find(".") {
                time_str = time_str[0..v].to_string();
            }
            let time = chrono::NaiveDateTime::parse_from_str(&time_str, "%Y_%m_%dT%H_%M_%S");
            if let Ok(time) = time {
                return Some(time);
            }
        }
        return None;
    }
}

/// split log file allow pack compress log
/// Memory space swop running time , reduces the number of repeated queries for IO
pub struct FileSplitAppenderData {
    max_split_bytes: usize,
    dir_path: String,
    file: File,
    sender: Sender<LogPack>,
    rolling_type: RollingType,
    //cache data
    temp_bytes: usize,
}

impl FileSplitAppenderData {
    pub fn send_pack(&mut self) {
        let first_file_path = format!("{}{}.log", self.dir_path, "temp");
        let new_log_name = format!(
            "{}{}{}.log",
            self.dir_path,
            "temp",
            format!("{:29}", Local::now().format("%Y_%m_%dT%H_%M_%S%.f")).replace(" ", "_")
        );
        std::fs::copy(&first_file_path, &new_log_name);
        self.sender.send(LogPack {
            dir: self.dir_path.clone(),
            rolling: self.rolling_type.clone(),
            new_log_name: new_log_name,
        });
        self.truncate();
    }

    pub fn truncate(&mut self) {
        //reset data
        self.file.set_len(0);
        self.file.seek(SeekFrom::Start(0));
        self.temp_bytes = 0;
    }
}

impl FileSplitAppender {
    ///split_log_bytes:  log file data bytes(MB) splite
    ///dir_path:         the log dir
    ///log_pack_cap:     pack(zip,lz4 or more...) or log Waiting cap
    /// packer: default is zip packer
    pub fn new(
        dir_path: &str,
        max_temp_size: LogSize,
        rolling_type: RollingType,
        log_pack_cap: usize,
        packer: Box<dyn Packer>,
    ) -> FileSplitAppender {
        if !dir_path.is_empty() && dir_path.ends_with(".log") {
            panic!("FileCompactionAppender only support new from path,for example: 'logs/xx/'");
        }
        if !dir_path.is_empty() && !dir_path.ends_with("/") {
            panic!("FileCompactionAppender only support new from path,for example: 'logs/xx/'");
        }
        if !dir_path.is_empty() {
            std::fs::create_dir_all(dir_path);
        }
        let first_file_path = format!("{}{}.log", dir_path, "temp");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(first_file_path.as_str());
        if file.is_err() {
            panic!(
                "[fast_log] open and create file fail:{}",
                file.err().unwrap()
            );
        }
        let mut file = file.unwrap();
        let mut temp_bytes = 0;
        if let Ok(m) = file.metadata() {
            temp_bytes = m.len() as usize;
        }
        file.seek(SeekFrom::Start(temp_bytes as u64));
        let (sender, receiver) = may::sync::mpsc::channel();
        spawn_saver(receiver, packer);
        Self {
            cell: RefCell::new(FileSplitAppenderData {
                max_split_bytes: max_temp_size.get_len(),
                temp_bytes: temp_bytes,
                dir_path: dir_path.to_string(),
                file: file,
                sender: sender,
                rolling_type: rolling_type,
            }),
        }
    }
}

impl LogAppender for FileSplitAppender {
    fn do_log(&self, record: &mut FastLogRecord) {
        let mut data = self.cell.borrow_mut();
        if record.command.eq(&Command::CommandFlush) || (data.temp_bytes >= data.max_split_bytes) {
            data.send_pack();
            return;
        }
        let mut write_bytes = 0;
        let w = data.file.write(record.formated.as_bytes());
        if let Ok(w) = w {
            write_bytes = write_bytes + w;
        }
        data.file.flush();
        data.temp_bytes += write_bytes;
    }
}

///spawn an saver thread to save log file or zip file
fn spawn_saver(r: Receiver<LogPack>, packer: Box<dyn Packer>) {
    std::thread::spawn(move || {
        loop {
            if let Ok(pack) = r.recv() {
                //do rolling
                pack.rolling.do_rolling(&pack.dir);
                let log_file_path = pack.new_log_name.clone();
                //do save pack
                let remove = do_pack(&packer, pack);
                if let Ok(remove) = remove {
                    if remove {
                        std::fs::remove_file(log_file_path);
                    }
                }
            }
        }
    });
}

/// write an Pack to zip file
pub fn do_pack(packer: &Box<dyn Packer>, mut pack: LogPack) -> Result<bool, LogPack> {
    let log_file_path = pack.new_log_name.as_str();
    if log_file_path.is_empty() {
        return Err(pack);
    }
    let log_file = OpenOptions::new().read(true).open(log_file_path);
    if log_file.is_err() {
        return Err(pack);
    }
    let log_file = log_file.unwrap();
    //make
    let r = packer.do_pack(log_file, log_file_path);
    if r.is_err() && packer.retry() > 0 {
        let mut retry = 1;
        while let Err(packs) = do_pack(packer, pack) {
            pack = packs;
            retry += 1;
            if retry > packer.retry() {
                break;
            }
        }
    }
    if let Ok(b) = r {
        return Ok(b);
    }
    return Ok(false);
}