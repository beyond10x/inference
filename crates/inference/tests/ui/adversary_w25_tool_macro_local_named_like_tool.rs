//! A local binding in the module of a tool, named like the tool.

/// The weather in a city.
#[inference::tool]
async fn weather(city: String) -> String {
    format!("sunny in {city}")
}

fn main() {
    let weather = "sunny";
    let _ = weather;
}
