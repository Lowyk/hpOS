#![no_std]
#![no_main]
mod runtime;
use hpos_desktop::{Decoder, pointer::MouseDecoder};

fn send(word: u64) {
    loop {
        let r = runtime::call(1, 0, word);
        if r.status == 0 {
            return;
        }
        if r.status != 1 {
            runtime::fail();
        }
        runtime::sleep(1);
    }
}
#[unsafe(no_mangle)]
extern "C" fn service_main() -> ! {
    if runtime::call(8, 0, 0).status != 3 {
        runtime::fail();
    }
    let mut keyboard = Decoder::new();
    let mut mouse = MouseDecoder::new(runtime::call(7, 1, 0).word as u8);
    let mut sequence = 1;
    let mut keyboard_loss = 0;
    let mut mouse_loss = 0;
    loop {
        let dropped = runtime::call(6, 1, 0).word;
        if dropped != keyboard_loss {
            keyboard.reset();
            keyboard_loss = dropped;
        }
        let dropped = runtime::call(7, 2, 0).word;
        if dropped != mouse_loss {
            mouse.reset();
            mouse_loss = dropped;
        }
        for _ in 0..128 {
            let raw = runtime::call(6, 0, 0);
            if raw.status == 1 {
                break;
            }
            if raw.status != 0 {
                runtime::fail();
            }
            if let Some(key) = keyboard.feed(raw.word as u8) {
                send(key.encode());
            }
        }
        for _ in 0..128 {
            let raw = runtime::call(7, 0, 0);
            if raw.status == 1 {
                break;
            }
            if raw.status != 0 {
                runtime::fail();
            }
            if let Some(packet) = mouse.feed(raw.word as u8) {
                send(packet.encode());
            }
        }
        runtime::report(sequence, 0);
        sequence = sequence.wrapping_add(1);
        runtime::sleep(1);
    }
}
