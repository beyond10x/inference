//! Cases of the adversary pass on the sentence a section of `quality/adapters.md` carries while no
//! adopted record holds its crate ([`NO_RECORD_YET`]).

use serde_json::Value;

use super::{ADAPTER_TESTS, NO_RECORD_YET, record_without, sentence_for_rows};
use crate::quality::{load, problems, record_problems, report};

/// A record whose own inputs name a package was written by a run whose plan named it, and
/// `assess` refuses a report without a test of each planned package. Such a record holding no
/// result of the package is not "a record before the crate was added": the sentence does not
/// excuse it, whatever its section says.
#[test]
fn a_record_whose_own_run_planned_the_crate_and_holds_no_result_of_it_is_refused_with_the_sentence()
 {
    let mut documents = load();
    record_without(&mut documents, "inference-aws");
    sentence_for_rows(&mut documents, "inference-aws");
    let record: Value = serde_json::from_str(&documents.records[ADAPTER_TESTS]).unwrap();
    let planned_by_its_run = record
        .pointer("/evidence/inputs/packages/value")
        .and_then(Value::as_array)
        .expect("the record names the packages of its run")
        .iter()
        .any(|package| package == "inference-aws");
    assert!(planned_by_its_run, "the run of the record planned inference-aws");
    for (rules, found) in [
        ("problems", problems(&documents)),
        ("record_problems", record_problems(&documents)),
    ] {
        assert!(
            found.iter().any(|problem| problem.contains("inference-aws")),
            "{rules}() names no problem of inference-aws, whose results were taken out of a record \
             that says its run planned it; {}",
            report(&found)
        );
    }
}

/// `quality/README.md`: "the sentence with a row ... [is] refused". A table under the sentence
/// whose first cells are not a measure in a code span is a row all the same, and its numbers are
/// backed by no record.
#[test]
fn the_sentence_with_a_table_whose_rows_name_no_measure_in_a_code_span_is_refused() {
    let mut documents = load();
    let sentence = format!("{NO_RECORD_YET}\n\nThe crate has no time bound");
    documents.adapters_page = documents.adapters_page.replace(
        &sentence,
        &format!(
            "{NO_RECORD_YET}\n\n| Measure | Baseline | Read from |\n|---|---|---|\n\
             | Tests run | 39 tests | Check run |\n| Tests passed | 39 tests | Check run |\n\n\
             The crate has no time bound"
        ),
    );
    assert!(
        documents.adapters_page.contains("| Tests run | 39 tests |"),
        "the table stands in the section of inference-sherpa-onnx-tts"
    );
    let found = problems(&documents);
    assert!(
        found
            .iter()
            .any(|problem| problem.contains("`inference-sherpa-onnx-tts`")),
        "no problem names the section `inference-sherpa-onnx-tts`, which says the sentence and \
         has a table of rows; {}",
        report(&found)
    );
}
