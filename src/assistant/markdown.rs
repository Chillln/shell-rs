//! 模型生成的 Markdown 只用于展示，图片与 HTML 不触发额外资源请求。

use markdown::{ParseOptions, mdast::Node};

pub(super) fn display_markdown(source: &str) -> String {
    let Ok(root) = markdown::to_mdast(source, &ParseOptions::gfm()) else {
        // 解析失败时按文字展示，避免不完整流式片段触发资源加载。
        return source.replace('<', "&lt;").replace("![", "&#33;[");
    };
    let mut replacements = Vec::new();
    collect(&root, source, &mut replacements);
    let mut result = source.to_owned();
    for (range, text) in replacements.into_iter().rev() {
        result.replace_range(range, &text);
    }
    result
}

fn collect(node: &Node, source: &str, replacements: &mut Vec<(std::ops::Range<usize>, String)>) {
    if let Some(position) = node.position() {
        let range = position.start.offset..position.end.offset;
        match node {
            Node::Image(_) | Node::ImageReference(_) => {
                // 仅替换图片标记，保留替代文字和地址供用户阅读。
                replacements.push((range.start..range.start + 1, "&#33;".into()));
            }
            Node::Html(_) => {
                replacements.push((range.clone(), source[range].replace('<', "&lt;")));
                return;
            }
            _ => {}
        }
    }
    if let Some(children) = node.children() {
        for child in children {
            collect(child, source, replacements);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::display_markdown;

    #[test]
    fn generated_images_and_html_cannot_load_resources_but_code_stays_exact() {
        let source = "**磁盘**\n\n![图](https://example.test/leak)\n\n![引用][img]\n\n[img]: https://example.test/leak\n\n<img src=\"https://example.test/leak\">\n\n```sh\ncat < input; echo '![raw](image)'\n```\n\n`x < y`";
        let safe = display_markdown(source);
        assert!(safe.contains("&#33;[图]"));
        assert!(safe.contains("&#33;[引用]"));
        assert!(safe.contains("&lt;img"));
        assert!(safe.contains("cat < input; echo '![raw](image)'"));
        assert!(safe.contains("`x < y`"));
        assert!(safe.contains("**磁盘**"));
    }
}
