//! 输入上下文快照：把 TSF 侧 / Rime 的组字状态暂存在 server 进程内，
//! 供面板绘制与 IPC 回包读取。

use std::sync::Mutex;

/// TSF 侧上报的组字串与光标 / 选区位置。
///
/// `cursor_pos` / `sel_start` / `sel_end` 是 TSF 原始数据的镜像：面板目前只
/// 整串绘制 preedit，原生光标定位与选区高亮还没做，所以这三个字段暂时只写
/// 不读——保留是为了不丢 TSF 已经给到的信息。
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct CompositionInfo {
    pub preedit: String,
    pub cursor_pos: usize,
    pub sel_start: usize,
    pub sel_end: usize,
}

/// Rime 候选的字面与注释（`InputContext::candidates` 的元素）。
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct CandidateInfo {
    pub text: String,
    pub comment: String,
}

#[derive(Debug, Clone, Default)]
pub struct SuggestionState {
    pub suggestions: Vec<String>,
    pub highlighted: usize,
    /// 触发这组建议的前缀串（安卓侧用于高亮命中部分），Windows 面板暂未绘制。
    #[allow(dead_code)]
    pub prefix_text: String,
}

#[derive(Debug, Clone, Default)]
pub struct InputContext {
    pub composition: CompositionInfo,
    pub candidates: Vec<CandidateInfo>,
    /// Rime 自己的分页信息：候选栏与面板各自维护分页状态，暂不读这两个值。
    #[allow(dead_code)]
    pub selected_index: usize,
    #[allow(dead_code)]
    pub page_size: usize,
    pub is_composing: bool,
    pub commit_text: String,
    pub caret_x: i32,
    pub caret_y: i32,
    /// 插入点所在行的高度（物理像素）。TSF 未上报时为 0，由候选栏布局回落默认行高。
    pub caret_height: i32,
    pub suggestion_state: Option<SuggestionState>,
}

impl InputContext {
    /// 候选栏定位锚点：`(插入点左边, 插入行下沿, 插入行高度)`，全部物理像素。
    pub fn caret_anchor(&self) -> (i32, i32, i32) {
        (self.caret_x, self.caret_y, self.caret_height)
    }
}

/// 跨线程共享的输入上下文（IPC 线程写、面板线程读）。
pub struct SharedInputContext {
    inner: Mutex<InputContext>,
}

impl Default for SharedInputContext {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedInputContext {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(InputContext::default()),
        }
    }

    /// 写入上下文。锁中毒时沿用最后一份数据（面板读改写的临界区很短，
    /// 中毒只可能来自 panic，没必要为此让整个 server 停摆）。
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut InputContext),
    {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard);
    }

    pub fn read<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&InputContext) -> R,
    {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }
}
