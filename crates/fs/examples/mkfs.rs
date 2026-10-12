//! Host tool: writes a formatted HPFS volume with the initial hpOS folders.
use hpos_fs::{Device, Error, Fs, Kind, ROOT, SECTOR};
use std::{env, fs, process};

struct Image(Vec<u8>);
impl Device for Image {
    fn read(&mut self, sector: u64, buffer: &mut [u8]) -> Result<(), Error> {
        let start = sector as usize * SECTOR;
        buffer.copy_from_slice(self.0.get(start..start + buffer.len()).ok_or(Error::Io)?);
        Ok(())
    }
    fn write(&mut self, sector: u64, buffer: &[u8]) -> Result<(), Error> {
        let start = sector as usize * SECTOR;
        self.0
            .get_mut(start..start + buffer.len())
            .ok_or(Error::Io)?
            .copy_from_slice(buffer);
        Ok(())
    }
}

const WELCOME: &str = "Это Хуйня Полнейшая ОС (hpOS) от Lowyk и tixcon.\n\
Файлы этого диска сохраняются между перезагрузками,\n\
если образ записан прямо на флешку.\n";

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 && args.len() != 4 {
        eprintln!("usage: mkfs <output> <sectors> [izzop-model]");
        process::exit(2);
    }
    let sectors: u64 = args[2].parse().expect("sector count");
    let mut volume = Box::new(Fs::new(Image(vec![0; sectors as usize * SECTOR])));
    volume.format(sectors, "hpOS").expect("format");
    let docs = volume
        .create(ROOT, "Документы", Kind::Directory)
        .expect("folder");
    let system = volume
        .create(ROOT, "Система", Kind::Directory)
        .expect("folder");
    if let Some(model) = args.get(3) {
        let bytes = fs::read(model).expect("izzop model");
        let file = volume
            .create(system, "izzop.bin", Kind::File)
            .expect("file");
        volume.write(file, &bytes).expect("write model");
    }
    let note = volume.create(docs, "Привет.txt", Kind::File).expect("file");
    volume.write(note, WELCOME.as_bytes()).expect("write");
    volume.mount().expect("verify");
    fs::write(&args[1], &volume.device().0).expect("output");
}
