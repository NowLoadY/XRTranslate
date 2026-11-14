use std::{
    fs,
    io::{Seek, Write},
    path::Path,
};
use xrtranslate_assets::{ModelPackage, PackageFile, identify_model_package};
use xrtranslate_download::DownloadCancellation;
use zip::write::SimpleFileOptions;

const FILES: [(&str, &[u8]); 2] = [
    ("weights.bin", b"model weights"),
    ("tokenizer/config.json", br#"{"language":"en"}"#),
];

fn package() -> ModelPackage {
    ModelPackage {
        id: "local-fixture".into(),
        revision: "v1".into(),
        archive: None,
        files: FILES
            .iter()
            .map(|(path, data)| PackageFile {
                path: (*path).into(),
                bytes: data.len() as u64,
                url: "https://example.invalid/never-requested".into(),
            })
            .collect(),
    }
}

fn folder(root: &Path) {
    for (name, data) in FILES {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }
}

fn archive(path: &Path, prefix: &str, extra: Option<&str>) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, data) in FILES {
        zip.start_file(format!("{prefix}{name}"), SimpleFileOptions::default())
            .unwrap();
        zip.write_all(data).unwrap();
    }
    if let Some(name) = extra {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(b"not a model file").unwrap();
    }
    zip.finish().unwrap();
}

fn import(
    source: &Path,
    target: &Path,
) -> Result<std::path::PathBuf, xrtranslate_assets::ModelDownloadError> {
    package().import(
        source,
        target,
        &DownloadCancellation::default(),
        |_, _, _| {},
    )
}

#[test]
fn the_builtin_catalog_can_be_checked_without_an_active_model_selection() {
    let root = tempfile::tempdir().unwrap();
    let packages: Vec<_> = xrtranslate_assets::MODEL_ASSET_CATALOG
        .iter()
        .map(ModelPackage::from)
        .collect();
    assert_eq!(
        identify_model_package(root.path(), &packages)
            .unwrap_err()
            .to_string(),
        "Unsupported model."
    );
}

#[test]
fn detection_matches_complete_folders_archives_and_individual_files() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("models/received");
    folder(&source);
    let zip = root.path().join("received.zip");
    archive(&zip, "models/received/", None);
    let expected = package();
    let mut other = package();
    other.id = "other-variant".into();
    other.files[0].bytes += 1;
    for path in [
        root.path().join("models"),
        zip,
        source.join(FILES[0].0),
        source.join(FILES[1].0),
    ] {
        assert_eq!(
            identify_model_package(&path, [&other, &expected]).unwrap(),
            &expected
        );
    }
    let target = root.path().join("imported");
    import(&source.join(FILES[0].0), &target).unwrap();
    assert!(expected.check(&target).unwrap());
}

#[test]
fn detection_rejects_unknown_files_and_incomplete_or_wrong_sized_models() {
    let root = tempfile::tempdir().unwrap();
    folder(root.path());
    let package = package();
    let unrelated = root.path().join("notes.txt");
    fs::write(&unrelated, "not a model").unwrap();
    assert_eq!(
        identify_model_package(&unrelated, [&package])
            .unwrap_err()
            .to_string(),
        "Unsupported model."
    );
    fs::remove_file(root.path().join(FILES[1].0)).unwrap();
    assert_eq!(
        identify_model_package(&root.path().join(FILES[0].0), [&package])
            .unwrap_err()
            .to_string(),
        "Unsupported model."
    );
    folder(root.path());
    fs::write(root.path().join(FILES[0].0), "wrong size").unwrap();
    assert_eq!(
        identify_model_package(root.path(), [&package])
            .unwrap_err()
            .to_string(),
        "Unsupported model."
    );
}

#[test]
fn detection_does_not_guess_between_models_and_selected_file_disambiguates() {
    let root = tempfile::tempdir().unwrap();
    folder(root.path());
    fs::write(root.path().join("other.gguf"), "weights").unwrap();
    let first = package();
    let second = ModelPackage {
        id: "second".into(),
        revision: "v1".into(),
        archive: None,
        files: vec![PackageFile {
            path: "other.gguf".into(),
            bytes: 7,
            url: "https://example.invalid/unused".into(),
        }],
    };
    assert_eq!(
        identify_model_package(root.path(), [&first, &second])
            .unwrap_err()
            .to_string(),
        "Multiple models found. Select one model file or folder."
    );
    assert_eq!(
        identify_model_package(&root.path().join("other.gguf"), [&first, &second]).unwrap(),
        &second
    );
}

#[test]
fn folder_import_uses_the_selected_storage_and_keeps_the_source() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("received/model");
    let target = root.path().join("custom-storage/models/different-name");
    folder(&source);
    fs::write(source.join("private-notes.txt"), b"keep at source").unwrap();
    import(&source, &target).unwrap();
    assert!(package().check(&target).unwrap());
    for (name, bytes) in FILES {
        assert_eq!(fs::read(source.join(name)).unwrap(), bytes);
        assert_eq!(fs::read(target.join(name)).unwrap(), bytes);
    }
    assert!(!target.join("private-notes.txt").exists());
    import(&target, &target).unwrap();
}

#[test]
fn zip_import_accepts_flat_model_and_models_parent_layouts() {
    for prefix in [
        "",
        "shared-model/",
        "models/shared-model/",
        "shared-model\\",
    ] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("received.zip");
        let target = root.path().join("storage/model");
        archive(&source, prefix, Some("__MACOSX/metadata"));
        let original = fs::read(&source).unwrap();
        import(&source, &target).unwrap();
        assert!(package().check(&target).unwrap());
        assert_eq!(fs::read(source).unwrap(), original);
        assert!(!target.join("__MACOSX").exists());
    }
}

#[test]
fn partial_install_is_completed_without_replacing_existing_files() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source/model");
    let target = root.path().join("storage/model");
    folder(&source);
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join(FILES[0].0), FILES[0].1).unwrap();
    fs::write(target.join("keep.txt"), b"user file").unwrap();
    import(&root.path().join("source"), &target).unwrap();
    assert!(package().check(&target).unwrap());
    assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"user file");
}

#[test]
fn incomplete_or_wrong_sized_sources_do_not_modify_the_destination() {
    for data in [None, Some(b"wrong size".as_slice())] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("storage/model");
        folder(&source);
        fs::remove_file(source.join(FILES[0].0)).unwrap();
        if let Some(data) = data {
            fs::write(source.join(FILES[0].0), data).unwrap();
        }
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep.txt"), b"untouched").unwrap();
        assert!(import(&source, &target).is_err());
        assert!(!target.join(FILES[0].0).exists());
        assert!(!target.join(FILES[1].0).exists());
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"untouched");
    }
}

#[test]
fn cancellation_discards_staging_without_publishing_a_partial_model() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let target = root.path().join("storage/model");
    folder(&source);
    let cancellation = DownloadCancellation::default();
    let result = package().import(&source, &target, &cancellation, |_, _, _| {
        cancellation.cancel()
    });
    assert!(result.unwrap_err().is_cancelled());
    assert!(!target.exists());
    assert!(package().check(&source).unwrap());
    assert_eq!(
        fs::read_dir(root.path().join("storage/.xrtranslate-staging"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn unsafe_or_ambiguous_archives_are_rejected_without_extracting_other_files() {
    for extra in ["../outside.txt", "/outside.txt", "weights.bin"] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("received.zip");
        // ZipWriter itself rejects duplicate entries; a second complete model is ambiguous too.
        if extra == "weights.bin" {
            let mut zip = zip::ZipWriter::new(fs::File::create(&source).unwrap());
            for prefix in ["first/", "second/"] {
                for (name, data) in FILES {
                    zip.start_file(format!("{prefix}{name}"), SimpleFileOptions::default())
                        .unwrap();
                    zip.write_all(data).unwrap();
                }
            }
            zip.finish().unwrap();
        } else {
            archive(&source, "model/", Some(extra));
        }
        let target = root.path().join("storage/model");
        assert!(import(&source, &target).is_err());
        assert!(!target.exists());
        assert!(!root.path().join("outside.txt").exists());
    }
}

#[test]
fn corrupt_zip_data_never_reaches_the_model_directory() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("received.zip");
    archive(&source, "", None);
    let offset = {
        let mut zip = zip::ZipArchive::new(fs::File::open(&source).unwrap()).unwrap();
        zip.by_name(FILES[0].0).unwrap().data_start()
    };
    let mut file = fs::OpenOptions::new().write(true).open(&source).unwrap();
    file.seek(std::io::SeekFrom::Start(offset)).unwrap();
    file.write_all(b"X").unwrap();
    drop(file);
    let target = root.path().join("storage/model");
    assert!(import(&source, &target).is_err());
    assert!(!target.exists());
}
