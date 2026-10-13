use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

/// 持有拖入压缩包时产生的临时文件，直到确认、排队和上传结束。
#[derive(Clone, Debug)]
pub struct StagingLease(Arc<StagingData>);

#[derive(Debug)]
struct StagingData {
    directory: tempfile::TempDir,
    originals: Vec<PathBuf>,
}

impl PartialEq for StagingLease {
    fn eq(&self, other: &Self) -> bool {
        self.0.directory.path() == other.0.directory.path()
    }
}

impl Eq for StagingLease {}

impl StagingLease {
    pub fn originals(&self) -> &[PathBuf] {
        &self.0.originals
    }
}

/// Windows 压缩文件夹通常在 Drop 返回时删掉临时解压目录，必须在回调中取得副本。
pub(super) fn stage_external_paths(
    paths: Vec<PathBuf>,
) -> io::Result<(Vec<PathBuf>, Option<StagingLease>)> {
    let temp_root = fs::canonicalize(std::env::temp_dir())?;
    stage_paths_in(paths, &temp_root)
}

fn stage_paths_in(
    paths: Vec<PathBuf>,
    temp_root: &Path,
) -> io::Result<(Vec<PathBuf>, Option<StagingLease>)> {
    let original_paths = paths.clone();
    let temporary: Vec<bool> = paths
        .iter()
        .map(|path| match fs::canonicalize(path) {
            Ok(canonical) => canonical.starts_with(temp_root),
            Err(_) => path.starts_with(std::env::temp_dir()),
        })
        .collect();
    if !temporary.iter().any(|temporary| *temporary) {
        return Ok((paths, None));
    }

    let directory = tempfile::Builder::new()
        .prefix("shellrs-upload-")
        .tempdir()?;
    let mut staged = Vec::with_capacity(paths.len());
    for (index, (source, temporary)) in paths.into_iter().zip(temporary).enumerate() {
        if !temporary {
            staged.push(source);
            continue;
        }
        let name = source
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "拖入的文件没有名称"))?;
        let parent = directory.path().join(index.to_string());
        fs::create_dir(&parent)?;
        let target = parent.join(name);
        copy_tree(&source, &target)?;
        staged.push(target);
    }
    Ok((
        staged,
        Some(StagingLease(Arc::new(StagingData {
            directory,
            originals: original_paths,
        }))),
    ))
}

fn copy_tree(source: &Path, target: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else if metadata.is_file() {
        // 同盘硬链接只增加目录项；跨盘或不支持硬链接时再复制内容。
        fs::hard_link(source, target).or_else(|_| fs::copy(source, target).map(|_| ()))?;
    } else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "不支持拖入的文件类型",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::stage_paths_in;
    use std::fs;

    #[test]
    fn archive_drop_survives_source_removal_and_cleans_up_after_release() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("解压目录");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("文件.txt"), "内容").unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        let (paths, lease) = stage_paths_in(vec![source.clone()], &root).unwrap();
        let lease = lease.unwrap();
        fs::remove_dir_all(source).unwrap();
        assert_eq!(lease.originals(), &[temporary.path().join("解压目录")]);
        assert_eq!(
            fs::read_to_string(paths[0].join("文件.txt")).unwrap(),
            "内容"
        );
        drop(lease);
        assert!(!paths[0].exists());
    }
}
