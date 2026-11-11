//! M6: 迁移自 IImageProvider.cs 中的 FishingAction / FishingEvent。

/// 对应 C# FishingAction 枚举。
///
/// Rust 标识符保持与 C# 对照以便维护，但**序列化名与 Debug 输出均为短码**：
/// 原始枚举名会随 serde derive 与 `derive(Debug)` 以明文进入二进制，
/// 属于公开样本中的可识别特征。线上格式 M/R/L/C 与前端 `ui/app.js` 一一对应。
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename = "E")]
pub enum FishingAction {
    #[serde(rename = "M")]
    BobberMove,
    #[serde(rename = "R")]
    Reset,
    #[serde(rename = "L")]
    Loot,
    #[serde(rename = "C")]
    Cast,
}

impl std::fmt::Debug for FishingAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FishingAction::BobberMove => "M",
            FishingAction::Reset => "R",
            FishingAction::Loot => "L",
            FishingAction::Cast => "C",
        })
    }
}

/// 对应 C# FishingEvent。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FishingEvent {
    pub action: FishingAction,
    pub amplitude: i32,
}

impl FishingEvent {
    pub fn new(action: FishingAction) -> Self {
        FishingEvent {
            action,
            amplitude: 0,
        }
    }
}

impl std::fmt::Display for FishingEvent {
    /// 与 C# FishingEvent.ToString() 对齐:
    /// `Action` 或 `Action Amplitude`（仅 BobberMove 带幅度值）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.action)?;
        if self.action == FishingAction::BobberMove {
            write!(f, " {}", self.amplitude)?;
        }
        Ok(())
    }
}
