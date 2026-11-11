//! M1: 迁移自 PixelClassifier.cs + ClassifierMode.cs。
//! 保真要点：C# 中为 byte * double 混合运算，Rust 显式转 f64，结果一致。

/// 对应 PixelClassifier.ClassifierMode。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifierMode {
    Red,
    Blue,
}

/// 对应 PixelClassifier（实现 IPixelClassifier）。
#[derive(Debug, Clone, PartialEq)]
pub struct PixelClassifier {
    pub colour_multiplier: f64,
    pub colour_closeness_multiplier: f64,
    pub mode: ClassifierMode,
}

impl Default for PixelClassifier {
    fn default() -> Self {
        PixelClassifier {
            colour_multiplier: 0.5,
            colour_closeness_multiplier: 2.0,
            mode: ClassifierMode::Red,
        }
    }
}

impl PixelClassifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_match(&self, red: u8, green: u8, blue: u8) -> bool {
        match self.mode {
            ClassifierMode::Red => {
                self.is_bigger(red, green)
                    && self.is_bigger(red, blue)
                    && self.are_close(blue, green)
            }
            ClassifierMode::Blue => {
                self.is_bigger(blue, green)
                    && self.is_bigger(blue, red)
                    && self.are_close(red, green)
            }
        }
    }

    /// 对应 SetConfiguration(bool isWowClasic)。
    pub fn set_configuration(&mut self, is_wow_classic: bool) {
        if is_wow_classic {
            tracing::info!("Wow Classic configuration");
            self.colour_multiplier = 1.0;
            self.colour_closeness_multiplier = 1.0;
        } else {
            tracing::info!("Wow Standard configuration");
        }
    }

    /// C#: (red * ColourMultiplier) > other
    fn is_bigger(&self, red: u8, other: u8) -> bool {
        (red as f64 * self.colour_multiplier) > other as f64
    }

    /// C#: min * ColourClosenessMultiplier > max - 20
    fn are_close(&self, color1: u8, color2: u8) -> bool {
        let max = color1.max(color2);
        let min = color1.min(color2);
        (min as f64 * self.colour_closeness_multiplier) > (max as f64 - 20.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 参照实现：直接按 C# 源码公式逐字翻译，用于对拍。
    fn reference_is_match(mode: ClassifierMode, cm: f64, ccm: f64, r: u8, g: u8, b: u8) -> bool {
        let is_bigger = |red: u8, other: u8| (red as f64 * cm) > other as f64;
        let are_close = |c1: u8, c2: u8| {
            let max = c1.max(c2);
            let min = c1.min(c2);
            (min as f64 * ccm) > (max as f64 - 20.0)
        };
        match mode {
            ClassifierMode::Red => is_bigger(r, g) && is_bigger(r, b) && are_close(b, g),
            ClassifierMode::Blue => is_bigger(b, g) && is_bigger(b, r) && are_close(r, g),
        }
    }

    #[test]
    fn matches_reference_across_sample_grid() {
        let clf = PixelClassifier::new();
        // 对 RGB 空间采样穷举（步长 17 覆盖 0..=255 全部边界值）
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(17) {
                    assert_eq!(
                        clf.is_match(r, g, b),
                        reference_is_match(ClassifierMode::Red, 0.5, 2.0, r, g, b),
                        "Red mode mismatch at ({r},{g},{b})"
                    );
                }
            }
        }
    }

    #[test]
    fn blue_mode_is_symmetric_counterpart() {
        let mut clf = PixelClassifier::new();
        clf.mode = ClassifierMode::Blue;
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(17) {
                    assert_eq!(
                        clf.is_match(r, g, b),
                        reference_is_match(ClassifierMode::Blue, 0.5, 2.0, r, g, b),
                        "Blue mode mismatch at ({r},{g},{b})"
                    );
                }
            }
        }
    }

    #[test]
    fn typical_bobber_red_matches() {
        let clf = PixelClassifier::new();
        // 典型浮漂红：R 远高于 G/B 且 G≈B
        assert!(clf.is_match(200, 60, 70));
        assert!(!clf.is_match(100, 100, 100)); // 灰色不匹配
        assert!(!clf.is_match(60, 200, 70)); // 绿色不匹配
    }

    #[test]
    fn classic_configuration_sets_multipliers_to_one() {
        let mut clf = PixelClassifier::new();
        clf.set_configuration(true);
        assert_eq!(clf.colour_multiplier, 1.0);
        assert_eq!(clf.colour_closeness_multiplier, 1.0);

        let mut clf2 = PixelClassifier::new();
        clf2.set_configuration(false);
        assert_eq!(clf2.colour_multiplier, 0.5);
        assert_eq!(clf2.colour_closeness_multiplier, 2.0);
    }
}
