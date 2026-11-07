//! A tool whose one argument is named like the tool itself: a note tool taking a note.

/// Saves a note.
#[inference::tool(effect = "idempotent_write")]
async fn note(note: String) -> String {
    note
}

fn main() {
    let _ = note;
}
