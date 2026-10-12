//! Desktop side of the file-system service protocol.
use crate::runtime;
use core::ptr::{addr_of_mut, copy_nonoverlapping, read_volatile, write_volatile};
use hpos_desktop::{
    files::{Entry, Name},
    lang::{Lang, pick},
};
use hpos_fs::Error;
use hpos_service_protocol::{
    FS_SHARED,
    fs::{self, Request},
};

/// Status reported when the service stops answering.
pub const TIMEOUT: u32 = 1000;
const REPLY_TICKS: u32 = 500;

pub struct Client {
    sequence: u64,
    alive: bool,
}
impl Client {
    pub const fn new() -> Self {
        Self {
            sequence: 0,
            alive: true,
        }
    }
    fn header() -> *mut Request {
        FS_SHARED as *mut Request
    }
    fn data() -> *mut u8 {
        (FS_SHARED as usize + fs::DATA_OFFSET) as *mut u8
    }
    pub fn call(
        &mut self,
        op: u32,
        args: [u64; 4],
        name: &str,
        data: &[u8],
    ) -> Result<[u64; 4], u32> {
        if !self.alive {
            return Err(TIMEOUT);
        }
        if name.len() > fs::NAME_BYTES || data.len() > fs::DATA_BYTES {
            return Err(Error::Invalid.code());
        }
        unsafe {
            let r = Self::header();
            write_volatile(addr_of_mut!((*r).op), op);
            write_volatile(addr_of_mut!((*r).status), u32::MAX);
            write_volatile(addr_of_mut!((*r).args), args);
            write_volatile(addr_of_mut!((*r).name_len), name.len() as u32);
            copy_nonoverlapping(
                name.as_ptr(),
                addr_of_mut!((*r).name).cast::<u8>(),
                name.len(),
            );
            copy_nonoverlapping(data.as_ptr(), Self::data(), data.len());
        }
        self.sequence = self.sequence.wrapping_add(1);
        let mut waited = 0;
        while runtime::call(1, 2, self.sequence).status == 1 {
            runtime::sleep(1);
            waited += 1;
            if waited > REPLY_TICKS {
                self.alive = false;
                return Err(TIMEOUT);
            }
        }
        loop {
            let reply = runtime::call(2, 3, 0);
            match reply.status {
                0 if reply.sender == 5 && reply.word == self.sequence => break,
                // A late reply to an abandoned request is discarded.
                0 if reply.sender == 5 => continue,
                1 => {
                    waited += 1;
                    if waited > REPLY_TICKS {
                        self.alive = false;
                        return Err(TIMEOUT);
                    }
                    runtime::sleep(1);
                }
                _ => runtime::fail(),
            }
        }
        let r = unsafe { read_volatile(Self::header()) };
        if r.status == 0 {
            Ok(r.results)
        } else {
            Err(r.status)
        }
    }
    pub fn read_data(&self, buffer: &mut [u8]) {
        let n = buffer.len().min(fs::DATA_BYTES);
        unsafe { copy_nonoverlapping(Self::data(), buffer.as_mut_ptr(), n) };
    }

    pub fn resolve(&mut self, path: &str) -> Result<u16, u32> {
        Ok(self.call(fs::RESOLVE, [0; 4], path, &[])?[0] as u16)
    }
    pub fn create(&mut self, parent: u16, name: &str, directory: bool) -> Result<u16, u32> {
        let kind = if directory { 2 } else { 1 };
        Ok(self.call(fs::CREATE, [parent as u64, kind, 0, 0], name, &[])?[0] as u16)
    }
    pub fn write(&mut self, id: u16, data: &[u8]) -> Result<(), u32> {
        self.call(fs::WRITE, [id as u64, data.len() as u64, 0, 0], "", data)
            .map(|_| ())
    }
    /// Reads the start of a file into `buffer`; returns the byte count.
    pub fn read(&mut self, id: u16, buffer: &mut [u8]) -> Result<usize, u32> {
        Ok(self.read_at(id, 0, buffer)?.0)
    }
    /// Reads from `offset`; returns (bytes read, file size).
    pub fn read_at(
        &mut self,
        id: u16,
        offset: usize,
        buffer: &mut [u8],
    ) -> Result<(usize, usize), u32> {
        let len = buffer.len().min(fs::DATA_BYTES);
        let r = self.call(fs::READ, [id as u64, offset as u64, len as u64, 0], "", &[])?;
        let n = (r[0] as usize).min(len);
        self.read_data(&mut buffer[..n]);
        Ok((n, r[1] as usize))
    }
    /// Lists a directory; returns how many entries were stored and whether more exist.
    pub fn list(&mut self, dir: u16, out: &mut [Entry]) -> Result<(usize, bool), u32> {
        let mut stored = 0;
        let mut after = fs::LIST_START;
        loop {
            let results = self.call(fs::LIST, [dir as u64, after, 0, 0], "", &[])?;
            let count = results[0] as usize;
            for index in 0..count {
                let raw = unsafe { read_volatile((Self::data() as *const fs::Entry).add(index)) };
                if stored == out.len() {
                    return Ok((stored, true));
                }
                out[stored] = Entry {
                    id: raw.id,
                    directory: raw.kind == 2,
                    size: raw.size,
                    name: Name::from_bytes(
                        &raw.name[..(raw.name_len as usize).min(raw.name.len())],
                    ),
                };
                stored += 1;
            }
            if results[2] == 0 {
                return Ok((stored, false));
            }
            after = results[1];
        }
    }
}

pub fn message(code: u32, lang: Lang) -> &'static str {
    if code == TIMEOUT {
        return pick(
            lang,
            [
                "Служба файлов не отвечает",
                "The file service does not answer",
                "Служба файлів не відповідає",
            ],
        );
    }
    pick(
        lang,
        match Error::from_code(code) {
            Some(Error::Exists) => [
                "Такое имя уже есть",
                "That name already exists",
                "Таке ім'я вже є",
            ],
            Some(Error::InvalidName) => ["Недопустимое имя", "Invalid name", "Неприпустиме ім'я"],
            Some(Error::NoSpace) => ["Нет места на диске", "Disk is full", "Немає місця на диску"],
            Some(Error::NoInodes) => ["Слишком много файлов", "Too many files", "Забагато файлів"],
            Some(Error::NotEmpty) => ["Папка не пуста", "Folder is not empty", "Тека не порожня"],
            Some(Error::NotFound) => ["Не найдено", "Not found", "Не знайдено"],
            Some(Error::NotDirectory) => ["Это не папка", "Not a folder", "Це не тека"],
            Some(Error::IsDirectory) => ["Это папка", "That is a folder", "Це тека"],
            Some(Error::Io) => [
                "Ошибка чтения или записи диска",
                "Disk read or write error",
                "Помилка читання або запису",
            ],
            Some(Error::NotMounted) => ["Диск недоступен", "Disk unavailable", "Диск недоступний"],
            Some(Error::TooLarge) => ["Файл слишком большой", "File too large", "Файл завеликий"],
            _ => [
                "Ошибка файловой системы",
                "File system error",
                "Помилка файлової системи",
            ],
        },
    )
}
