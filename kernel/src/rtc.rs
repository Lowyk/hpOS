//! CMOS real-time clock, read as the firmware keeps it (usually local time).

fn read(register: u8) -> u8 {
    crate::out(0x70, register);
    crate::input(0x71)
}
fn updating() -> bool {
    read(0x0a) & 0x80 != 0
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Raw([u8; 6]);
fn snapshot() -> Raw {
    Raw([
        read(0x00),
        read(0x02),
        read(0x04),
        read(0x07),
        read(0x08),
        read(0x09),
    ])
}

/// Returns `(time, date)` status words, bit 31 set when the reading is plausible.
pub fn now() -> (u64, u64) {
    // Read until two snapshots outside an update cycle agree.
    let mut previous = None;
    for _ in 0..16 {
        for _ in 0..10_000 {
            if !updating() {
                break;
            }
        }
        let current = snapshot();
        if previous == Some(current) {
            return decode(current, read(0x0b), read(0x32));
        }
        previous = Some(current);
    }
    (0, 0)
}

fn decode(raw: Raw, status_b: u8, century: u8) -> (u64, u64) {
    let binary = status_b & 0x04 != 0;
    let value = |v: u8| {
        if binary {
            v
        } else {
            (v & 0x0f) + (v >> 4) * 10
        }
    };
    let [second, minute, hour_raw, day, month, year] = raw.0;
    let pm = hour_raw & 0x80 != 0;
    let mut hour = value(hour_raw & 0x7f);
    if status_b & 0x02 == 0 {
        // 12-hour mode: 12 AM is 0, 12 PM stays 12.
        hour = (hour % 12) + if pm { 12 } else { 0 };
    }
    let (second, minute, day, month) = (value(second), value(minute), value(day), value(month));
    let century = match value(century) {
        19..=21 => value(century) as u64,
        _ => 20,
    };
    let year = century * 100 + value(year) as u64;
    let valid_time = hour < 24 && minute < 60 && second < 60;
    let valid_date = (1..=31).contains(&day) && (1..=12).contains(&month);
    (
        if valid_time {
            1 << 31 | (hour as u64) << 16 | (minute as u64) << 8 | second as u64
        } else {
            0
        },
        if valid_date {
            1 << 31 | year << 16 | (month as u64) << 8 | day as u64
        } else {
            0
        },
    )
}
