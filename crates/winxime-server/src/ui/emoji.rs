//! 候选栏面板「表情」页的内置表情表（纯数据）。
//!
//! 数据是代码内常量：离线可用、零配置、不依赖 rime 方案的 emoji 词典部署
//! （面板只负责「点一下就上屏」，不做输入编码联想）。
//!
//! 版式与寻址在 `ui::glyph` 里：本模块只提供「有哪些分类、每类有哪些字形」。
//! 分类超过一页时由 `ui::glyph` 在分类内翻页，所以这里不限制每类个数。
//!
//! 取值原则：只用 Segoe UI Emoji（Win10 1809+）稳定有字形的常见表情，
//! 不追新（Unicode 14+ 的新表情在旧系统上会画成方框）；不用 ZWJ 组合序列
//! （👨‍👩‍👧 之类）——那些在格子里会被挤成一团，且宽度不稳定（安卓版有这类序列，
//! 桌面网格里刻意不收）。

use super::glyph::GlyphCategory;

pub(crate) const CATEGORIES: &[GlyphCategory] = &[
    GlyphCategory {
        label: "常用",
        glyphs: &[
            "😀", "😄", "😁", "😂", "🤣", "😊", "😍", "🥰",
            "😘", "😎", "🤔", "🙄", "😅", "😇", "😉", "😌",
            "😴", "😭", "😢", "😡", "😱", "🥺", "🤗", "🤝",
            "🙏", "👍", "👌", "✌️", "👏", "🎉", "❤️", "🔥",
        ],
    },
    GlyphCategory {
        label: "人物",
        glyphs: &[
            "😃", "😆", "☺️", "🙂", "🙃", "😋", "😛", "😜",
            "🤪", "😝", "🤭", "🤫", "🤐", "😐", "😑", "😶",
            "😏", "😒", "😞", "😔", "😟", "😕", "🙁", "😣",
            "😖", "😫", "😩", "🥺", "😤", "😠", "🤬", "😈",
        ],
    },
    GlyphCategory {
        label: "手势",
        glyphs: &[
            "👍", "👎", "👌", "✌️", "🤞", "🤟", "🤘", "🤙",
            "👈", "👉", "👆", "👇", "☝️", "✋", "🤚", "🖐️",
            "🖖", "👋", "🤝", "🙏", "💪", "🤛", "🤜", "👏",
            "🙌", "👐", "🤲", "✊", "👊", "💅", "🤳", "🙋",
        ],
    },
    GlyphCategory {
        label: "自然",
        glyphs: &[
            "🌸", "🌹", "🌺", "🌻", "🌷", "🌱", "🌲", "🌳",
            "🌴", "🌵", "🌾", "🌿", "🍀", "🍁", "🍂", "🍃",
            "🌍", "🌎", "🌏", "🌙", "⭐", "🌟", "✨", "⚡",
            "☀️", "⛅", "☁️", "🌧️", "⛈️", "❄️", "🌈", "🌊",
        ],
    },
    GlyphCategory {
        label: "食物",
        glyphs: &[
            "🍎", "🍐", "🍊", "🍋", "🍌", "🍉", "🍇", "🍓",
            "🍒", "🍑", "🥭", "🍍", "🥥", "🥝", "🍅", "🥑",
            "🥦", "🥕", "🌽", "🌶️", "🥒", "🍞", "🥐", "🥖",
            "🧀", "🥚", "🍳", "🥓", "🍔", "🍕", "🍟", "🌭",
        ],
    },
    GlyphCategory {
        label: "符号",
        glyphs: &[
            "❤️", "🧡", "💛", "💚", "💙", "💜", "🖤", "🤍",
            "💔", "❣️", "💕", "💞", "💓", "💗", "💖", "💘",
            "💝", "💟", "✅", "❌", "⭕", "❗", "❓", "⚠️",
            "♻️", "🔰", "⚜️", "🔱", "📛", "♠️", "♥️", "♦️",
        ],
    },
];
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_category_is_the_common_one() {
        // 打开表情页默认落在「常用」（最近使用是它前面的第 0 个标签，见 ui::glyph）。
        assert_eq!(CATEGORIES.first().map(|cat| cat.label), Some("常用"));
    }

    #[test]
    fn every_glyph_is_a_single_emoji_character() {
        // 格子里只画一个表情：1~2 个 char（变体选择符 FE0F 是第 2 个）。
        // 空白/ZWJ 组合序列由 ui::glyph 的通用不变量测试兜（它同时盯符号表）。
        for cat in CATEGORIES {
            for emoji in cat.glyphs {
                let chars: Vec<char> = emoji.chars().collect();
                assert!(
                    (1..=2).contains(&chars.len()),
                    "{} 里的 {} 是 {} 个 char，格子放不下",
                    cat.label,
                    emoji,
                    chars.len()
                );
                assert!(
                    !emoji.contains('\u{200d}'),
                    "{} 里的 {} 是 ZWJ 组合序列，网格里会挤成一团",
                    cat.label,
                    emoji
                );
            }
        }
    }

    #[test]
    fn common_category_holds_the_most_used_glyphs() {
        let common = CATEGORIES.first();
        assert!(common.is_some());
        let count = common.map_or(0, |cat| cat.glyphs.len());
        assert!(count >= 16, "「常用」至少要有两行常用表情，现在只有 {count} 个");
    }
}