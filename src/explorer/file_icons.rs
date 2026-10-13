//! SFTP 文件列表中的图标：按设置选择界面单色图标或 Material Icon Theme 彩色图标。
//! 彩色图标根据文件类型和常见文件夹名称匹配。

use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use super::file_icons_table::{
    DEFAULT_FILE, DEFAULT_FOLDER, FILE_EXTENSIONS, FILE_EXTENSIONS_LIGHT, FILE_NAMES,
    FILE_NAMES_LIGHT, FOLDER_NAMES, FOLDER_NAMES_LIGHT,
};
use super::{FileEntry, FileKind};
use crate::app::CatalogIcon;

/// 图标风格默认单色，可在设置中切换为彩色；与 `FileSizeFormat` 一样存为全局状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileIconStyle {
    #[default]
    Monochrome,
    Color,
}

impl Global for FileIconStyle {}

impl FileIconStyle {
    pub fn current(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }
}

/// 文件列表行对应的图标类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileIconKind {
    /// 上级目录 `..`。
    Parent,
    Folder,
    File,
    /// 指向文件夹的符号链接。
    FolderLink,
    /// 指向文件的符号链接，或已失效的符号链接。
    FileLink,
}

impl FileIconKind {
    fn is_folder(self) -> bool {
        matches!(self, Self::Parent | Self::Folder | Self::FolderLink)
    }

    fn is_link(self) -> bool {
        matches!(self, Self::FolderLink | Self::FileLink)
    }
}

/// 按设置渲染文件或文件夹图标。彩色填充图形略大于单色线条图标；
/// 符号链接在左下角叠加箭头。
#[derive(IntoElement)]
pub struct FileIcon {
    name: SharedString,
    kind: FileIconKind,
    dimmed: bool,
}

impl FileIcon {
    pub fn new(name: impl Into<SharedString>, kind: FileIconKind) -> Self {
        Self {
            name: name.into(),
            kind,
            dimmed: false,
        }
    }

    /// 根据文件列表行创建图标。
    pub fn entry(entry: &FileEntry) -> Self {
        let kind = if entry.is_parent() {
            FileIconKind::Parent
        } else {
            match (entry.kind, entry.target) {
                (FileKind::Dir, _) => FileIconKind::Folder,
                (FileKind::File, _) => FileIconKind::File,
                (FileKind::Symlink, Some(FileKind::Dir)) => FileIconKind::FolderLink,
                (FileKind::Symlink, _) => FileIconKind::FileLink,
            }
        };
        Self::new(entry.name.clone(), kind)
    }

    /// 隐藏文件使用较淡的图标。
    pub fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self
    }
}

impl RenderOnce for FileIcon {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let kind = self.kind;
        match FileIconStyle::current(cx) {
            FileIconStyle::Monochrome => match kind {
                FileIconKind::Parent => Icon::new(CatalogIcon::FolderUp),
                FileIconKind::Folder => Icon::new(IconName::Folder),
                FileIconKind::File => Icon::new(IconName::File),
                FileIconKind::FolderLink => Icon::new(CatalogIcon::FolderSymlink),
                FileIconKind::FileLink => Icon::new(CatalogIcon::FileSymlink),
            }
            .small()
            .flex_none()
            .text_color(cx.theme().muted_foreground)
            .when(self.dimmed, |icon| icon.opacity(0.6))
            .into_any_element(),
            FileIconStyle::Color => {
                let path = color_icon(&self.name, kind, !cx.theme().is_dark());
                div()
                    .id("color-icon")
                    .relative()
                    .flex_none()
                    .size_4()
                    .when(self.dimmed, |icon| icon.opacity(0.6))
                    // 图像按自身比例在固定尺寸内绘制。
                    .child(img(path).size_4())
                    .when(kind.is_link(), |icon| {
                        icon.child(
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size_2p5()
                                .rounded_sm()
                                .bg(cx.theme().background)
                                .child(
                                    Icon::new(CatalogIcon::ArrowUpRight)
                                        .size_2()
                                        .text_color(cx.theme().foreground),
                                ),
                        )
                    })
                    .test_support()
                    .into_any_element()
            }
        }
    }
}

/// 匹配 Material Icon Theme 彩色资源：文件夹按名称；文件先按名称，再按最长扩展名；
/// 名称不区分大小写。浅色主题优先使用浅色变体，`..` 使用普通文件夹图标。
fn color_icon(name: &str, kind: FileIconKind, light: bool) -> &'static str {
    if kind == FileIconKind::Parent {
        return DEFAULT_FOLDER;
    }
    let name = name.to_lowercase();
    let find = |light_table: &'static [(&'static str, &'static str)],
                table: &'static [(&'static str, &'static str)],
                key: &str| {
        let search = |table: &'static [(&'static str, &'static str)]| {
            table
                .binary_search_by(|(entry, _)| (*entry).cmp(key))
                .ok()
                .map(|ix| table[ix].1)
        };
        light
            .then(|| search(light_table))
            .flatten()
            .or_else(|| search(table))
    };
    if kind.is_folder() {
        return find(FOLDER_NAMES_LIGHT, FOLDER_NAMES, &name).unwrap_or(DEFAULT_FOLDER);
    }
    find(FILE_NAMES_LIGHT, FILE_NAMES, &name)
        .or_else(|| {
            name.match_indices('.')
                .map(|(ix, _)| &name[ix + 1..])
                .filter(|extension| !extension.is_empty())
                .find_map(|extension| find(FILE_EXTENSIONS_LIGHT, FILE_EXTENSIONS, extension))
        })
        .unwrap_or(DEFAULT_FILE)
}

#[cfg(test)]
mod tests {
    use super::super::file_icons_table::*;
    use super::{FileIconKind, color_icon};

    fn icon(name: &str, kind: FileIconKind) -> &'static str {
        color_icon(name, kind, false)
            .strip_prefix("icons/files/")
            .and_then(|file| file.strip_suffix(".svg"))
            .unwrap()
    }

    #[test]
    fn files_are_known_by_name_then_by_their_longest_extension() {
        use FileIconKind::{File, FileLink};
        assert_eq!(icon(".bashrc", File), "console");
        assert_eq!(icon("deploy.sh", File), "console");
        assert_eq!(icon("Dockerfile", File), "docker");
        assert_eq!(icon("docker-compose.yml", File), "docker");
        assert_eq!(icon("config.yaml", File), "yaml");
        assert_eq!(icon("nginx.conf", File), "nginx");
        assert_eq!(icon("php.ini", File), "settings");
        assert_eq!(icon("README.md", File), "readme");
        assert_eq!(icon("notes.MD", File), "markdown");
        assert_eq!(icon("backup.tar.gz", File), "zip");
        assert_eq!(icon(".env", File), "tune");
        assert_eq!(icon("shellrs.service", File), "systemd");
        assert_eq!(icon("id_ed25519", File), "key");
        assert_eq!(icon("server.key", File), "key");
        assert_eq!(icon(".vimrc", File), "vim");
        assert_eq!(icon("sshd_config", File), "lock");
        // 符号链接按自身名称匹配。
        assert_eq!(icon("python3.py", FileLink), "python");
        // 未识别的名称使用普通文件图标。
        assert_eq!(icon(".lesshst", File), "file");
        assert_eq!(icon("notes", File), "file");
        assert_eq!(icon("archive.", File), "file");
    }

    #[test]
    fn folders_are_known_by_name() {
        use FileIconKind::{Folder, FolderLink, Parent};
        assert_eq!(icon(".config", Folder), "folder-config");
        assert_eq!(icon(".ssh", Folder), "folder-secure");
        assert_eq!(icon("Downloads", Folder), "folder-download");
        assert_eq!(icon("node_modules", Folder), "folder-node");
        assert_eq!(icon("logs", FolderLink), "folder-log");
        // 文件夹名称不按扩展名匹配。
        assert_eq!(icon("app.py", Folder), "folder");
        // 服务器上的 /dev 和 /media 不是项目开发或媒体目录。
        assert_eq!(icon("dev", Folder), "folder");
        assert_eq!(icon("media", Folder), "folder");
        assert_eq!(icon("..", Parent), "folder");
        assert_eq!(icon("AppData", Folder), "folder");
    }

    #[test]
    fn a_light_background_takes_the_light_variant() {
        let kind = FileIconKind::File;
        assert_eq!(
            color_icon("hosts", kind, true),
            "icons/files/hosts_light.svg"
        );
        assert_eq!(color_icon("hosts", kind, false), "icons/files/hosts.svg");
        assert_eq!(
            color_icon("Cargo.toml", kind, true),
            "icons/files/toml_light.svg"
        );
        // 大多数图标在浅色和深色主题中共用。
        assert_eq!(
            color_icon("a.sh", kind, true),
            color_icon("a.sh", kind, false)
        );
    }

    #[test]
    fn the_tables_can_be_searched_and_every_icon_is_in_the_bundle() {
        let tables = [
            FOLDER_NAMES,
            FOLDER_NAMES_LIGHT,
            FILE_NAMES,
            FILE_NAMES_LIGHT,
            FILE_EXTENSIONS,
            FILE_EXTENSIONS_LIGHT,
        ];
        let bundled = |path: &str| FILE_ICON_ASSETS.iter().any(|(asset, _)| *asset == path);
        for table in tables {
            for pair in table.windows(2) {
                assert!(pair[0].0 < pair[1].0, "未排序或重复：{pair:?}");
            }
            for (key, path) in table {
                assert_eq!(*key, key.to_lowercase(), "键要小写：{key}");
                assert!(bundled(path), "{path} 不在资源包里");
            }
        }
        assert!(bundled(DEFAULT_FILE) && bundled(DEFAULT_FOLDER));
        for (path, bytes) in FILE_ICON_ASSETS {
            let svg = std::str::from_utf8(bytes).unwrap();
            assert!(
                svg.starts_with(r#"<svg width="16" height="16""#),
                "{path} 没有按 16 像素生成"
            );
        }
    }
}
