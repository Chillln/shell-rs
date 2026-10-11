//! One-shot remote file operations: delete, rename, create, change
//! permissions. Trees are walked with an explicit stack and symbolic links
//! are never followed.

use super::{EntryKind, RemoteOperation, RemotePath, client::RemoteFs};
use crate::i18n::t;
use anyhow::{Result, bail};

pub(crate) async fn run<F: RemoteFs>(fs: &F, operation: &RemoteOperation) -> Result<()> {
    match operation {
        RemoteOperation::Delete {
            paths,
            confirmation,
        } => {
            verify_delete_confirmation(paths, confirmation.as_deref())?;
            for path in paths {
                delete_tree(fs, path).await?;
            }
            Ok(())
        }
        RemoteOperation::Rename { from, to } => {
            if fs.metadata(to).await?.is_some() {
                bail!(t!("sftp.error.exists", name = to.file_name()));
            }
            fs.rename(from, to).await
        }
        RemoteOperation::CreateDirectory { path } => {
            if fs.metadata(path).await?.is_some() {
                bail!(t!("sftp.error.exists", name = path.file_name()));
            }
            fs.mkdir(path).await
        }
        RemoteOperation::CreateFile { path } => {
            let handle = fs.open(path, true).await?;
            fs.close(&handle).await
        }
        RemoteOperation::SetPermissions {
            paths,
            edit,
            recursive,
            add_x_to_dirs,
        } => {
            // Parents first, so a change that grants access lets the walk in.
            let mut stack: Vec<RemotePath> = paths.iter().rev().cloned().collect();
            while let Some(path) = stack.pop() {
                let Some(metadata) = fs.metadata(&path).await? else {
                    continue;
                };
                let is_dir = metadata.kind() == EntryKind::Directory;
                if metadata.kind() == EntryKind::Symlink {
                    continue;
                }
                if let Some(mode) = metadata.permissions() {
                    let next = edit.apply(mode, is_dir, *add_x_to_dirs);
                    if next != mode {
                        fs.set_permissions(&path, next & 0o7777).await?;
                    }
                }
                if *recursive && is_dir {
                    let mut children = fs.read_dir(&path).await?;
                    children.sort_by(|a, b| b.name().cmp(a.name()));
                    for child in children {
                        stack.push(path.join(child.name())?);
                    }
                }
            }
            Ok(())
        }
    }
}

/// Delete a path and, for a directory, everything under it: children before
/// their directory. A link is removed as a link.
pub(super) async fn delete_tree<F: RemoteFs>(fs: &F, root: &RemotePath) -> Result<()> {
    let mut stack = vec![(root.clone(), false)];
    while let Some((path, emptied)) = stack.pop() {
        if emptied {
            fs.rmdir(&path).await?;
            continue;
        }
        match fs.metadata(&path).await? {
            None => {}
            Some(metadata) if metadata.kind() == EntryKind::Directory => {
                stack.push((path.clone(), true));
                for child in fs.read_dir(&path).await? {
                    stack.push((path.join(child.name())?, false));
                }
            }
            Some(_) => fs.remove(&path).await?,
        }
    }
    Ok(())
}

/// 不允许直接删除这些目录；其子项必须输入完整路径确认。
const PROTECTED_ROOTS: &[&str] = &[
    "/bin", "/boot", "/dev", "/etc", "/lib", "/lib64", "/nix", "/proc", "/root", "/run", "/sbin",
    "/sys", "/usr", "/var", "/Library", "/System", "/private",
];

/// 判断远端删除是否需要手动输入完整路径；关键目录本身直接拒绝。
pub fn delete_confirmation(paths: &[RemotePath]) -> Result<Option<String>> {
    if paths.is_empty() {
        bail!(t!("sftp.error.delete_path"));
    }
    let mut required = None;
    for path in paths {
        let raw = path.as_str();
        if !raw.starts_with('/') || raw.starts_with("//") || raw.ends_with('/') {
            bail!(t!("sftp.error.delete_path"));
        }
        let parts: Vec<_> = raw.split('/').filter(|part| !part.is_empty()).collect();
        if parts.is_empty() || parts.iter().any(|part| matches!(*part, "." | "..")) {
            bail!(t!("sftp.error.delete_path"));
        }
        let normalized = format!("/{}", parts.join("/"));
        if PROTECTED_ROOTS.contains(&normalized.as_str()) {
            bail!(t!("sftp.error.delete_protected", path = raw));
        }
        let root = format!("/{}", parts[0]);
        let needs_typing = PROTECTED_ROOTS.contains(&root.as_str())
            || ["/home", "/Users"].contains(&root.as_str()) && parts.len() <= 2
            || ["/mnt", "/media", "/Volumes"].contains(&root.as_str())
            || ["/tmp", "/opt", "/srv", "/Applications"].contains(&normalized.as_str());
        if needs_typing {
            required = Some(raw.to_string());
        }
    }
    if required.is_some() && paths.len() != 1 {
        bail!(t!("sftp.error.delete_one_at_a_time"));
    }
    Ok(required)
}

/// 执行时再核对确认文字，防止弹窗打开后目标路径发生变化。
pub(super) fn verify_delete_confirmation(
    paths: &[RemotePath],
    confirmation: Option<&str>,
) -> Result<()> {
    if let Some(required) = delete_confirmation(paths)?
        && confirmation != Some(required.as_str())
    {
        bail!(t!("sftp.error.delete_confirmation", path = required));
    }
    Ok(())
}

/// 将绝对路径变成单个 POSIX shell 参数，并执行同一套删除保护。
pub(super) fn force_delete_command(
    paths: &[RemotePath],
    confirmation: Option<&str>,
) -> Result<String> {
    verify_delete_confirmation(paths, confirmation)?;
    let mut command = String::from("rm -rf --");
    for path in paths {
        let raw = path.as_str();
        command.push_str(" '");
        command.push_str(&raw.replace('\'', r"'\''"));
        command.push('\'');
    }
    Ok(command)
}

#[cfg(test)]
mod force_delete_tests {
    use super::{RemotePath, delete_confirmation, force_delete_command};

    #[test]
    fn quotes_each_remote_path_as_one_shell_argument() {
        let paths = [
            RemotePath::new("/home/u/a b;$HOME").unwrap(),
            RemotePath::new("/home/u/it's here").unwrap(),
        ];
        assert_eq!(
            force_delete_command(&paths, None).unwrap(),
            "rm -rf -- '/home/u/a b;$HOME' '/home/u/it'\\''s here'"
        );
    }

    #[test]
    fn refuses_root_relative_and_dot_paths() {
        for path in ["/", "////", "relative", "/a/../b", "/a/./b", "/a/"] {
            assert!(force_delete_command(&[RemotePath::new(path).unwrap()], None).is_err());
        }
        assert!(force_delete_command(&[], None).is_err());
    }

    #[test]
    fn blocks_critical_directories_even_with_confirmation() {
        for path in ["/etc", "/usr", "/var", "/System", "/etc//"] {
            let target = RemotePath::new(path).unwrap();
            assert!(delete_confirmation(std::slice::from_ref(&target)).is_err());
            assert!(force_delete_command(&[target], Some(path)).is_err());
        }
    }

    #[test]
    fn critical_children_and_home_directories_require_exact_path() {
        for path in ["/etc/ssh", "/var/www", "/home/user", "/mnt/data"] {
            let target = RemotePath::new(path).unwrap();
            assert_eq!(
                delete_confirmation(std::slice::from_ref(&target)).unwrap(),
                Some(path.into())
            );
            assert!(force_delete_command(std::slice::from_ref(&target), None).is_err());
            assert!(force_delete_command(std::slice::from_ref(&target), Some("wrong")).is_err());
            assert!(force_delete_command(&[target], Some(path)).is_ok());
        }
        let ordinary = RemotePath::new("/home/user/project").unwrap();
        assert_eq!(delete_confirmation(&[ordinary]).unwrap(), None);
    }

    #[test]
    fn high_risk_items_must_be_deleted_one_at_a_time() {
        let paths = [
            RemotePath::new("/etc/ssh").unwrap(),
            RemotePath::new("/etc/hosts").unwrap(),
        ];
        assert!(delete_confirmation(&paths).is_err());
    }
}
