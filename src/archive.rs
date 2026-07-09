use std::{
    fs::{self, File, OpenOptions},
    io::{self, IsTerminal, Seek, Write, stdout},
    path::{Component, Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::Args;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::loading;

#[derive(Debug, Args)]
pub struct ArchiveArgs {
    /// Directory to archive. Defaults to the current directory.
    #[arg(long, value_name = "PATH")]
    pub path: Option<PathBuf>,
}

pub fn run(args: ArchiveArgs) -> Result<()> {
    let source = args
        .path
        .unwrap_or(std::env::current_dir().context("failed to resolve current directory")?);
    let destination = if stdout().is_terminal() {
        loading::run_with_spinner("Creating zip archive", move || archive_path(&source))?
    } else {
        archive_path(&source)?
    };

    println!("Created {}", destination.display());
    Ok(())
}

fn archive_path(source: &Path) -> Result<PathBuf> {
    let source = source
        .canonicalize()
        .with_context(|| format!("failed to resolve source path {}", source.display()))?;

    if !source.is_dir() {
        bail!("archive source must be a directory: {}", source.display());
    }

    let destination = unique_archive_path(&source);
    let temp = unique_temp_archive_path()?;

    if let Err(error) = write_zip_archive(&source, &temp, &[destination.clone(), temp.clone()]) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }

    if let Err(error) = copy_archive_into_place(&temp, &destination) {
        let _ = fs::remove_file(&temp);
        let _ = fs::remove_file(&destination);
        return Err(error);
    }

    Ok(destination)
}

fn write_zip_archive(source: &Path, temp: &Path, excluded: &[PathBuf]) -> Result<()> {
    if let Some(parent) = temp.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create temporary directory {}", parent.display())
        })?;
    }

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)
        .with_context(|| format!("failed to create temporary archive {}", temp.display()))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    add_directory_contents(&mut zip, source, source, excluded, options)?;
    zip.finish().context("failed to finish zip archive")?;
    Ok(())
}

fn add_directory_contents<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    source: &Path,
    directory: &Path,
    excluded: &[PathBuf],
    options: SimpleFileOptions,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)
        .with_context(|| format!("failed to read directory {}", directory.display()))?
        .collect::<std::result::Result<Vec<_>, io::Error>>()
        .with_context(|| format!("failed to enumerate directory {}", directory.display()))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        if excluded.iter().any(|excluded| *excluded == path) {
            continue;
        }

        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read metadata for {}", path.display()))?;
        let relative = path
            .strip_prefix(source)
            .with_context(|| format!("failed to relativize {}", path.display()))?;
        let name = zip_entry_name(relative);

        if file_type.is_symlink() {
            continue;
        } else if file_type.is_dir() {
            zip.add_directory(format!("{name}/"), options)
                .with_context(|| {
                    format!("failed to add directory {} to archive", path.display())
                })?;
            add_directory_contents(zip, source, &path, excluded, options)?;
        } else if file_type.is_file() {
            zip.start_file(&name, options)
                .with_context(|| format!("failed to add file {} to archive", path.display()))?;
            let mut file = File::open(&path)
                .with_context(|| format!("failed to open file {}", path.display()))?;
            io::copy(&mut file, zip)
                .with_context(|| format!("failed to write file {} to archive", path.display()))?;
        }
    }

    Ok(())
}

fn copy_archive_into_place(temp: &Path, destination: &Path) -> Result<()> {
    let mut source = File::open(temp)
        .with_context(|| format!("failed to open temporary archive {}", temp.display()))?;
    let mut target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("failed to create archive {}", destination.display()))?;

    io::copy(&mut source, &mut target)
        .with_context(|| format!("failed to copy archive to {}", destination.display()))?;

    if let Err(error) = fs::remove_file(temp) {
        eprintln!(
            "warning: failed to remove temporary archive {}: {error}",
            temp.display()
        );
    }

    Ok(())
}

fn unique_archive_path(source: &Path) -> PathBuf {
    let base = source
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("archive");

    let mut counter = 1;
    loop {
        let filename = if counter == 1 {
            format!("{base}.zip")
        } else {
            format!("{base}-{counter}.zip")
        };
        let candidate = source.join(filename);

        if !candidate.exists() {
            return candidate;
        }

        counter += 1;
    }
}

fn unique_temp_archive_path() -> Result<PathBuf> {
    let temp_dir = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|_| std::env::temp_dir());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut counter = 1;

    loop {
        let candidate = temp_dir.join(format!(
            "ps-archive-{}-{stamp}-{counter}.zip",
            process::id()
        ));

        if !candidate.exists() {
            return Ok(candidate);
        }

        counter += 1;
    }
}

fn zip_entry_name(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::ZipArchive;

    #[test]
    fn chooses_a_new_archive_name_without_overwriting() {
        let root = make_test_dir("names");
        let base = root.file_name().unwrap().to_string_lossy();
        fs::write(root.join(format!("{base}.zip")), b"existing").unwrap();

        let destination = unique_archive_path(&root);

        assert_eq!(
            destination.file_name().unwrap().to_string_lossy(),
            format!("{base}-2.zip")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn creates_zip_from_directory_contents() {
        let root = make_test_dir("contents");
        fs::write(root.join("alpha.txt"), b"alpha").unwrap();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("nested").join("beta.txt"), b"beta").unwrap();

        let destination = archive_path(&root).unwrap();
        let archive_file = File::open(&destination).unwrap();
        let archive = ZipArchive::new(archive_file).unwrap();
        let names = archive.file_names().map(str::to_owned).collect::<Vec<_>>();
        let archive_name = destination
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        assert!(names.contains(&"alpha.txt".to_string()));
        assert!(names.contains(&"nested/".to_string()));
        assert!(names.contains(&"nested/beta.txt".to_string()));
        assert!(!names.contains(&archive_name));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn uses_forward_slashes_for_zip_entries() {
        assert_eq!(
            zip_entry_name(Path::new("nested").join("file.txt").as_path()),
            "nested/file.txt"
        );
    }

    fn make_test_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("ps-archive-test-{label}-{}-{stamp}", process::id()));

        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }
}
