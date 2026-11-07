//! `#[inference::main]` takes the host and the stack; a `main` without the stack is refused.

#[inference::main]
async fn main(host: impl inference::audio_device::AudioHost) {
    let _ = host;
}
