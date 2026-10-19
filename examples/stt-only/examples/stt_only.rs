//! Prints each span the mock recogniser commits: `cargo run -p stt-only --example stt_only`.

fn main() {
    for line in stt_only::run() {
        println!("{line}");
    }
}
