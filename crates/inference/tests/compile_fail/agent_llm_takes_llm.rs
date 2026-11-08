//! The role is in the type: `AgentBuilder::llm` takes a `Bound<role::Llm>`, so a `tts` binding is
//! refused at compile time.

use inference::core::model::ModelId;
use inference::{Agent, Binding};
use inference_testkit::duplex::mock::pcm;
use inference_testkit::tts::{MockConfiguration, MockSynthesiser};

fn main() {
    let synthesiser = MockSynthesiser::new(
        MockConfiguration::IncrementalStreaming,
        ModelId::generate(),
        pcm(24_000),
    );
    let _ = Agent::voice().llm(Binding::tts(synthesiser));
}
