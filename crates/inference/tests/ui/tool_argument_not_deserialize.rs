//! A tool argument must be `Deserialize`: the model's JSON arguments are read into it.

struct Opaque;

/// Takes an argument no JSON can be read into.
#[inference::tool]
async fn opaque(value: Opaque) -> String {
    let _ = value;
    String::new()
}

fn main() {}
