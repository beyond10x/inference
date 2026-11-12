//! Security pass on story:elevenlabs-tts-provider-cancel: every public API change is listed under
//! `### Breaking` in `CHANGELOG.md`. The change adds the variant `Measure::TtsOfferToAudio` to the
//! public enum `Measure` of the `realtime-voice` example (so `Measure::ALL` becomes `[Measure; 8]`)
//! and the public field `tts_segment_first_audio` to its public struct `Measured`, neither marked
//! `#[non_exhaustive]`: an exhaustive match on `Measure`, a binding typed `[Measure; 7]` and a
//! struct literal of `Measured` stop compiling. The 0.28.0 section's `Breaking` list, the release
//! that shipped the change, already records such changes of an unpublished example
//! (`local-voice-cascade`'s `CascadeIds`).

/// The `### Breaking` list of the 0.28.0 section.
fn release_breaking() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../CHANGELOG.md");
    let changelog = std::fs::read_to_string(path).expect("the changelog");
    let release = changelog
        .split("\n## ")
        .find(|section| section.starts_with("[0.28.0]"))
        .expect("a 0.28.0 section");
    let breaking = release
        .split("\n### ")
        .find(|section| section.starts_with("Breaking"))
        .expect("a Breaking list");
    breaking.to_owned()
}

#[test]
fn the_new_measure_variant_and_measured_field_of_realtime_voice_are_listed_as_breaking() {
    let breaking = release_breaking();
    assert!(
        breaking.contains("realtime-voice") && breaking.contains("Measure"),
        "the 0.28.0 Breaking list does not name the realtime-voice `Measure` and `Measured` \
         changes:\n{breaking}"
    );
}
