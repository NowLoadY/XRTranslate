use std::{
    fs,
    io::{Seek, Write},
    path::Path,
};
use xrtranslate_assets::{ModelPackage, PackageFile};
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
