//! Two tools in one module, one of them taking an argument named like the other.

/// The city the user is in.
#[inference::tool]
async fn city() -> String {
    "Oslo".to_owned()
}

/// The weather in a city.
#[inference::tool]
async fn weather(city: String) -> String {
    format!("sunny in {city}")
}

fn main() {
    let _ = (city, weather);
}
