use super::*;
use crate::matters::{
    document_output::append_text,
    output_navigation::navigation_evidence,
    page_tools::assembly::{self, LinkTarget, Navigation, Overlay, Source},
};
use std::collections::{BTreeMap, HashMap};

const BUILD: &str = "11111111111111111111111111111111";

fn output(labels: &[&str]) -> Vec<u8> {
    let source = Source::open(assembly::blank((612.0, 792.0)).unwrap()).unwrap();
    let navigation = Navigation {
        labels: labels.iter().map(|label| label.to_string()).collect(),
        ..Default::default()
    };
    assembly::assemble_bundle(&[source], &[(0, 0), (0, 0)], &HashMap::new(), &navigation).unwrap()
}

fn linked_output() -> Vec<u8> {
    let source = Source::open(assembly::blank((612.0, 792.0)).unwrap()).unwrap();
    let navigation = Navigation {
        labels: vec!["i".into(), "Annexure (अ)-2".into()],
        outlines: vec![(1, "Synthetic target (अ)".into(), 1)],
        links: vec![
            (0, [20.0, 30.0, 50.0, 60.0], LinkTarget::Page(1)),
            (
                1,
                [30.0, 45.0, 60.0, 70.0],
                LinkTarget::Uri("https://example.invalid/review".into()),
            ),
        ],
        ..Default::default()
    };
    let mut overlay = Overlay::default();
    append_text(
        &mut overlay,
        "Helvetica",
        12.0,
        40.0,
        100.0,
        (0.0, 0.0, 0.0),
        b"Synthetic linked evidence",
    );
    assembly::assemble_bundle(
        &[source],
        &[(0, 0), (0, 0)],
        &HashMap::from([(1, overlay)]),
        &navigation,
    )
    .unwrap()
}

fn check<'a>(run: &'a Value, key: &str) -> &'a Value {
    run["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == key)
        .unwrap()
}

#[test]
fn a_missing_label_tree_uses_physical_viewer_labels() {
    let pdf = PdfDocument::from_bytes(output(&[])).unwrap();
    let mut spec = json!({"pages":2,"toc":[],"links":{},"labels":["1","2"],
        "printed_labels":["",""]});
    assert!(!pdf
        .catalog()
        .unwrap()
        .as_dict()
        .unwrap()
        .contains_key("PageLabels"));
    for legacy in [false, true] {
        let measured = super::super::output_navigation::inspect(&pdf, 2, &spec, legacy).unwrap();
        assert!(measured.labels);
        assert!(measured.navigation);
    }
    spec["labels"] = json!([]);
    let older = super::super::output_navigation::inspect(&pdf, 2, &spec, false).unwrap();
    assert!(older.labels);
    spec["toc"] = Value::Null;
    let missing = super::super::output_navigation::inspect(&pdf, 2, &spec, false).unwrap();
    assert!(!missing.navigation);
    assert!(missing.labels);
}

#[test]
fn rewriting_cannot_change_link_coordinates_or_zoom_on_the_same_page() {
    let original = linked_output();
    let expected = PdfDocument::from_bytes(original.clone()).unwrap();
    let expected = navigation_evidence(&expected, 2).unwrap();
    let xyz = b"/XYZ 0 792 0]";
    let offset = original
        .windows(xyz.len())
        .position(|part| part == xyz)
        .unwrap();
    for changed in [b"/XYZ 0 791 0]", b"/XYZ 0 792 2]"] {
        let mut candidate = original.clone();
        // Equal-width operands leave the fixture's xref offsets unchanged.
        candidate[offset..offset + xyz.len()].copy_from_slice(changed);
        let candidate = PdfDocument::from_bytes(candidate).unwrap();
        let candidate = navigation_evidence(&candidate, 2).unwrap();
        assert_ne!(expected, candidate);
        assert!(!expected.matches_serialized(&candidate));
    }
}

#[test]
fn completed_protected_export_keeps_text_and_unicode_navigation() {
    use base64::{engine::general_purpose::STANDARD, Engine};

    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(&directory.path().join("store")).unwrap();
    let created = store.request(
        "POST",
        "/matters",
        json!({"name":"Synthetic protected output","court_profile":"general-demo"}),
    );
    assert_eq!(created.status, 201, "{created:?}");
    let matter = created.body["id"].as_str().unwrap();
    let original = linked_output();
    let manifest = json!({
        "id":BUILD,"archived_at":null,"stale":false,
        "files":[{"name":"combined.pdf","bytes":original.len(),"sha256":sha256_hex(&original)}],
        "validation_contract":{"combined.pdf":{"pages":2,"toc":[[1,"Synthetic target (अ)",2]],
            "links":{"0":[{"page":1}],"1":[{"uri":"https://example.invalid/review"}]},
            "labels":["i","Annexure (अ)-2"],"printed_labels":["",""]}},
    });
    let initial_check = measure_build(matter, &manifest, None, |_| Ok(original.clone())).unwrap();
    assert_eq!(
        check(&initial_check, "navigation")["outcome"],
        "passed",
        "{initial_check}"
    );
    assert_eq!(
        check(&initial_check, "labels")["outcome"],
        "passed",
        "{initial_check}"
    );
    publish(
        &store.con,
        matter,
        BUILD,
        Build {
            manifest: manifest.clone(),
            files: BTreeMap::from([("combined.pdf".into(), original.clone())]),
            initial_check,
        },
    )
    .unwrap();
    let response = store.request(
        "POST",
        &format!("/matters/{matter}/exports/{BUILD}/combined.pdf/protected"),
        json!({"password":"synthetic-secret"}),
    );
    assert_eq!(response.status, 200, "{response:?}");
    assert_eq!(
        response.body["content_disposition"],
        "attachment; filename=\"combined-protected.pdf\""
    );
    let encrypted = PdfDocument::from_bytes(
        STANDARD
            .decode(response.body["data"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(encrypted.is_encrypted());
    assert!(!encrypted.authenticate(b"wrong").unwrap());
    assert!(encrypted.authenticate(b"synthetic-secret").unwrap());
    let source = PdfDocument::from_bytes(original.clone()).unwrap();
    assert_eq!(
        navigation_evidence(&encrypted, 2),
        navigation_evidence(&source, 2)
    );
    assert!(encrypted
        .extract_text(1)
        .unwrap()
        .contains("Synthetic linked evidence"));
    assert_eq!(
        file(&store.con, &manifest, "combined.pdf").unwrap(),
        original
    );
}

#[test]
fn current_checks_link_wrong_viewer_labels_and_preserve_the_full_run() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(&directory.path().join("store")).unwrap();
    let matter = store.request(
        "POST",
        "/matters",
        json!({"name":"Synthetic validation run"}),
    );
    assert_eq!(matter.status, 201);
    let matter = matter.body["id"].as_str().unwrap();
    let imported = store.import_pdf(matter, "source.pdf", PILOT_PDF_BYTES.to_vec());
    assert_eq!(imported.status, 201, "{imported:?}");
    let doc = imported.body[0]["id"].as_str().unwrap();
    let files = BTreeMap::from([
        ("combined.pdf".to_owned(), output(&["1", "2"])),
        (
            "index.csv".to_owned(),
            format!("{}\r\n", super::super::bundle_writer::CSV_FIELDS.join(",")).into_bytes(),
        ),
    ]);
    let entries = files
        .iter()
        .map(|(name, bytes)| json!({"name":name,"bytes":bytes.len(),"sha256":sha256_hex(bytes)}))
        .collect::<Vec<_>>();
    let manifest = json!({
        "id":BUILD,"files":entries,"archived_at":null,"stale":false,"page_map_snapshot":[],
        "validation_contract":{"combined.pdf":{"pages":2,"toc":[],"links":{},
            "labels":["1","wrong"],"printed_labels":["",""],
            "sources":{"1":{"document_id":doc,"page_index":0,"target_revision":1}}}},
    });
    let initial_check =
        measure_build(matter, &manifest, None, |name| Ok(files[name].clone())).unwrap();
    publish(
        &store.con,
        matter,
        BUILD,
        Build {
            manifest,
            files,
            initial_check,
        },
    )
    .unwrap();
    let path = format!("/matters/{matter}/exports/{BUILD}");
    let result = store.request("POST", &format!("{path}/validation/run"), json!({}));
    assert_eq!(result.status, 200, "{result:?}");
    let full = &result.body["run"];
    assert_eq!(check(full, "files")["outcome"], "passed");
    assert_eq!(check(full, "navigation")["outcome"], "passed");
    assert_eq!(check(full, "labels")["outcome"], "failed");
    let finding = &check(full, "labels")["findings"][0];
    assert_eq!(finding["document_id"], doc);
    assert_eq!(finding["page_index"], 0);
    assert_eq!(finding["target_revision"], 1);
    assert_eq!(finding["output_page_index"], 1);
    assert_eq!(
        finding["message"],
        "combined.pdf: physical page 2, expected printed/viewer label wrong"
    );
    assert_eq!(result.body["can_download"], false);
    let run = full["id"].as_str().unwrap();
    let finding_id = finding["finding_id"].as_str().unwrap();
    let record = store.request(
        "GET",
        &format!("/matters/{matter}/validation/findings/{run}/{finding_id}"),
        Value::Null,
    );
    assert_eq!(record.status, 200, "{record:?}");
    assert_eq!(record.body["applicable"], true);
    let subset = store.request(
        "POST",
        &format!("{path}/validation/run"),
        json!({"files":["index.csv"]}),
    );
    assert_eq!(subset.status, 200, "{subset:?}");
    assert_eq!(subset.body["run"]["id"], run);
    assert_eq!(
        check(&subset.body["history"][0], "labels")["outcome"],
        "not-applicable"
    );
    assert_eq!(subset.body["can_download"], false);
    assert_eq!(
        store
            .request("GET", &format!("{path}/combined.pdf"), Value::Null)
            .status,
        409
    );
    assert_eq!(
        store
            .request(
                "GET",
                &format!("{path}/inspection/combined.pdf"),
                Value::Null
            )
            .status,
        200
    );
    store.con.execute(
        "UPDATE rust_export_files SET content=zeroblob(length(content)) WHERE export_id=? AND filename='combined.pdf'",
        [BUILD],
    ).unwrap();
    let damaged = store.request("POST", &format!("{path}/validation/run"), json!({}));
    assert_eq!(damaged.status, 200, "{damaged:?}");
    assert_eq!(
        damaged.body["run"]["measured_files"]["combined.pdf"],
        json!({"error":ARTIFACT_UNAVAILABLE})
    );
    assert_eq!(check(&damaged.body["run"], "files")["findings"], json!([]));
    assert_eq!(
        check(&damaged.body["run"], "navigation")["evidence"]["summary"],
        "combined.pdf: could not parse PDF"
    );
    let preview = store.request(
        "GET",
        &format!("{path}/inspection/combined.pdf"),
        Value::Null,
    );
    assert_eq!(preview.status, 404, "{preview:?}");
    assert_eq!(preview.body["detail"], "Saved PDF not found");
}
