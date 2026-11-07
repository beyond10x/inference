//! A tool with typed arguments, one returning `Result`, and the `effect` and `confirm` options,
//! in a program whose only dependency is `inference`.

use std::fmt;

/// Adds two whole numbers.
#[inference::tool]
async fn add(a: i64, b: i64) -> i64 {
    a + b
}

#[derive(Debug)]
struct Refused;

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("refused")
    }
}

/// Books a table, after the user confirms it.
#[inference::tool(effect = "non_idempotent_write", confirm)]
pub async fn book(day: String, seats: u8) -> Result<String, Refused> {
    if seats == 0 {
        return Err(Refused);
    }
    Ok(format!("{seats} seats on {day}"))
}

/// Saves a note.
#[inference::tool(effect = "idempotent_write")]
async fn save(note: Vec<String>) {
    let _ = note;
}

fn main() {}
