//! Host tool: builds the Izzop model from a cleaned message corpus, one message per line.
use std::{env, fs, process};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: train <corpus.txt> <model.bin>");
        process::exit(2);
    }
    let corpus = fs::read_to_string(&args[1]).expect("corpus");
    let model = hpos_izzop::build::train(&corpus);
    hpos_izzop::Model::parse(&model).expect("valid model");
    fs::write(&args[2], &model).expect("output");
    println!("Izzop model: {} bytes", model.len());
}
