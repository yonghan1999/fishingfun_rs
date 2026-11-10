//! M6: 迁移自 IImageProvider.cs 中的 FishingAction / FishingEvent。

/// 对应 C# FishingAction 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FishingAction {
    BobberMove,
    Reset,
    Loot,
    Cast,
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
