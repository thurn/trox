use std::fs;

use assert_cmd::Command;
use tempfile::tempdir;

#[test]
fn quest_scenarios_expand_across_four_grammatically_distinct_locales() {
    let target = tempdir().unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stress/quest");
    copy_dir(&source, target.path());
    let config = target.path().join("trox.ron");
    Command::cargo_bin("trox")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "extract"])
        .assert()
        .success();
    Command::cargo_bin("trox")
        .unwrap()
        .args(["--config", config.to_str().unwrap(), "check"])
        .assert()
        .success();

    let counts = ["ar", "es", "ja", "ru"].map(|locale| {
        let content = fs::read_to_string(
            target
                .path()
                .join(format!("generated/locales/{locale}.csv")),
        )
        .unwrap();
        (locale, content.lines().count() - 1, content)
    });
    let ar = counts
        .iter()
        .find(|(locale, _, _)| *locale == "ar")
        .unwrap();
    let es = counts
        .iter()
        .find(|(locale, _, _)| *locale == "es")
        .unwrap();
    let ja = counts
        .iter()
        .find(|(locale, _, _)| *locale == "ja")
        .unwrap();
    let ru = counts
        .iter()
        .find(|(locale, _, _)| *locale == "ru")
        .unwrap();
    assert!(
        ar.1 > ru.1 && ru.1 > ja.1,
        "CLDR category expansion should differ"
    );
    assert!(es.2.contains("subtype_noun.gender=feminine"));
    assert!(!ja.2.contains(".gender="));
    assert!(ru.2.contains("plural=few") && ru.2.contains("plural=many"));
    assert!(ar.2.contains("plural=zero") && ar.2.contains("plural=two"));

    Command::cargo_bin("trox")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "bundle",
            "--allow-missing",
        ])
        .assert()
        .success();
    let source_bundle = trox::Bundle::from_canonical_json(
        &fs::read_to_string(target.path().join("generated/bundles/en-US.trox.json")).unwrap(),
    )
    .unwrap();
    let arabic_bundle = trox::Bundle::from_canonical_json(
        &fs::read_to_string(target.path().join("generated/bundles/ar.trox.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(arabic_bundle.direction, trox::TextDirection::Rtl);
    trox::Localizer::new(arabic_bundle, source_bundle).unwrap();
}

fn copy_dir(source: &std::path::Path, target: &std::path::Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "generated" {
            continue;
        }
        let output = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &output);
        } else {
            fs::copy(entry.path(), output).unwrap();
        }
    }
}
