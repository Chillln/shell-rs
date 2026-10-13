#!/usr/bin/env python3
"""从 Material Icon Theme（MIT）挑选 SFTP 文件列表使用的彩色图标。

仅打包下方 ICONS 列出的图标，以及主题为它们配置的文件名、扩展名和文件夹名。
图标及许可证写入 assets/icons/files/，映射表写入
src/explorer/file_icons_table.rs，再使用 rustfmt 格式化。

    npm pack material-icon-theme@<version>
    mkdir material && tar xzf material-icon-theme-<version>.tgz -C material
    python3 -I scripts/file_icons.py material/package

要更新主题映射时，使用新版主题包重新运行。
"""

import json
import re
import subprocess
import sys
from pathlib import Path

# 只打包服务器、部署项目和用户目录常见的图标；未收录的类型使用普通图标。
ICONS = [
    # 文件。
    "file", "document", "log", "console", "settings", "tune", "json", "yaml",
    "xml", "toml", "markdown", "readme", "license", "changelog", "authors",
    "contributing", "table", "pdf", "word", "powerpoint", "epub", "email",
    "zip", "jar", "javaclass", "image", "svg", "audio", "video", "subtitles",
    "font", "key", "certificate", "lock", "database", "exe", "dll", "lib",
    "hex", "disc", "docker", "git", "diff", "nginx", "hosts", "systemd", "vim",
    "python", "python-misc", "jupyter", "javascript", "typescript", "react",
    "react_ts", "nodejs", "npm", "yarn", "vue", "html", "css", "sass", "less",
    "java", "kotlin", "scala", "gradle", "maven", "go", "go-mod", "rust", "c",
    "cpp", "h", "hpp", "csharp", "php", "ruby", "gemfile", "lua", "perl", "r",
    "swift", "dart", "powershell", "makefile", "cmake", "terraform", "hcl",
    "nix", "proto", "graphql", "android", "jinja", "editorconfig", "vagrant",
    "jenkins", "robots", "favicon",
    # 文件夹。
    "folder", "folder-config", "folder-scripts", "folder-secure", "folder-keys",
    "folder-private", "folder-git", "folder-github", "folder-home",
    "folder-temp", "folder-log", "folder-src", "folder-lib", "folder-dist",
    "folder-target", "folder-test", "folder-docs", "folder-images",
    "folder-audio", "folder-video", "folder-download", "folder-upload",
    "folder-desktop", "folder-public", "folder-database", "folder-backup",
    "folder-archive", "folder-trash", "folder-node", "folder-python",
    "folder-environment", "folder-docker", "folder-kubernetes", "folder-helm",
    "folder-ansible", "folder-terraform", "folder-nginx", "folder-redis",
    "folder-server", "folder-client", "folder-api", "folder-app",
    "folder-packages", "folder-plugin", "folder-include", "folder-resource",
    "folder-views", "folder-template", "folder-font", "folder-mail",
    "folder-tools", "folder-utils", "folder-vscode", "folder-intellij",
    "folder-go", "folder-rust", "folder-gradle", "folder-project",
    "folder-link", "folder-other",
]

# 补充服务器常见的 systemd、SSH 和 vim 文件映射；主题自带映射优先。
EXTRA_FILE_EXTENSIONS = {
    **{ext: "systemd" for ext in [
        "service", "socket", "timer", "mount", "automount", "slice", "target",
    ]},
}
EXTRA_FILE_NAMES = {
    "ssh_config": "lock",
    "sshd_config": "lock",
    **{name: "key" for name in [
        "authorized_keys", "known_hosts", "id_rsa", "id_dsa", "id_ecdsa",
        "id_ed25519",
    ]},
    **{name: "vim" for name in [".vimrc", "_vimrc", ".gvimrc", ".exrc", ".viminfo"]},
}
EXTRA_FOLDER_NAMES = {
    ".ssh": "folder-secure",
    ".gnupg": "folder-secure",
}

# 普通文件夹改为黄色，接近 Windows、WinSCP 和 Xftp；专用文件夹保留原色。
FOLDER_COLOR = "#ffca28"
THEME_FOLDER_COLOR = "#90a4ae"

# 服务器上的 /dev 和 /media 含义不同于项目目录，排除名称匹配。
EXCLUDED_FOLDER_NAMES = {"dev", "media"}

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets" / "icons" / "files"
TABLE = ROOT / "src" / "explorer" / "file_icons_table.rs"
ASSET_PREFIX = "icons/files/"


def main(package: Path) -> None:
    manifest = json.loads((package / "dist" / "material-icons.json").read_text())
    version = json.loads((package / "package.json").read_text())["version"]
    definitions = manifest["iconDefinitions"]
    missing = [icon for icon in ICONS if icon not in definitions]
    if missing:
        sys.exit(f"not in the theme: {', '.join(missing)}")
    shipped = set(ICONS)

    def base(icon: str) -> str:
        return icon.removesuffix("_light")

    def associations(table: dict, extra: dict, excluded=frozenset()) -> dict:
        found = {
            key.lower(): icon
            for key, icon in table.items()
            if base(icon) in shipped and "/" not in key and key.lower() not in excluded
        }
        for key, icon in extra.items():
            found.setdefault(key, icon)
        return dict(sorted(found.items()))

    light = manifest["light"]
    tables = {
        "FOLDER_NAMES": associations(
            manifest["folderNames"], EXTRA_FOLDER_NAMES, EXCLUDED_FOLDER_NAMES
        ),
        "FOLDER_NAMES_LIGHT": associations(
            light.get("folderNames", {}), {}, EXCLUDED_FOLDER_NAMES
        ),
        "FILE_NAMES": associations(manifest["fileNames"], EXTRA_FILE_NAMES),
        "FILE_NAMES_LIGHT": associations(light.get("fileNames", {}), {}),
        "FILE_EXTENSIONS": associations(
            manifest["fileExtensions"], EXTRA_FILE_EXTENSIONS
        ),
        "FILE_EXTENSIONS_LIGHT": associations(light.get("fileExtensions", {}), {}),
    }
    used = sorted(
        {manifest["file"], manifest["folder"]}
        | {icon for table in tables.values() for icon in table.values()}
    )

    ASSETS.mkdir(parents=True, exist_ok=True)
    for old in ASSETS.glob("*.svg"):
        old.unlink()
    for icon in used:
        source = (package / "dist" / definitions[icon]["iconPath"]).resolve()
        svg = sized(source.read_text())
        if icon == manifest["folder"]:
            if THEME_FOLDER_COLOR not in svg:
                sys.exit("the plain folder's color has changed")
            svg = svg.replace(THEME_FOLDER_COLOR, FOLDER_COLOR)
        (ASSETS / f"{icon}.svg").write_text(svg)
    (ASSETS / "LICENSE").write_text((package / "LICENSE").read_text())

    TABLE.write_text(rust_tables(version, manifest, tables, used))
    subprocess.run(["rustfmt", "--edition", "2024", str(TABLE)], check=True)
    print(f"{len(used)} icons from Material Icon Theme {version}")


def sized(svg: str) -> str:
    """将图标绘制为 16 像素，确保 GPUI 在一倍和两倍缩放下清晰。"""
    svg = svg.strip()
    match = re.match(r"<svg\b([^>]*)>", svg)
    if not match:
        sys.exit("not an SVG")
    attributes = re.sub(r'\s(width|height)="[^"]*"', "", match.group(1))
    return f'<svg width="16" height="16"{attributes}>{svg[match.end():]}\n'


def rust_tables(version: str, manifest: dict, tables: dict, used: list) -> str:
    def path(icon: str) -> str:
        return f'"{ASSET_PREFIX}{icon}.svg"'

    lines = [
        f"//! 由 `scripts/file_icons.py` 从 Material Icon Theme {version} 生成。",
        "//! 许可见 `assets/icons/files/LICENSE`；请勿手动修改。",
        "//!",
        "//! 键统一为小写并排序，以便二分查找；`_LIGHT` 表保存浅色主题变体。",
        "",
        f"pub(super) const DEFAULT_FILE: &str = {path(manifest['file'])};",
        f"pub(super) const DEFAULT_FOLDER: &str = {path(manifest['folder'])};",
        "",
    ]
    for name, table in tables.items():
        lines.append(f"pub(super) const {name}: &[(&str, &str)] = &[")
        lines += [f"    ({json.dumps(key, ensure_ascii=False)}, {path(icon)})," for key, icon in table.items()]
        lines += ["];", ""]
    lines.append("/// 应用资源源使用的图标列表。")
    lines.append("pub const FILE_ICON_ASSETS: &[(&str, &[u8])] = &[")
    for icon in used:
        lines.append(
            f'    ({path(icon)}, include_bytes!("../../assets/icons/files/{icon}.svg")),'
        )
    lines += ["];", ""]
    return "\n".join(lines)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(Path(sys.argv[1]))
