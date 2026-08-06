//! 装甲系统通用类型定义模块
//!
//! 定义了装甲类型 (ArmorType)、装甲标签 (ArmorLabel)、装甲规格 (ArmorSpec) 等核心枚举，
//! 以及贴纸插槽 (ArmorStickerSlot) 结构体和对应的常量数组。
//! 提供了小装甲/大装甲标签到 ArmorLabel 的转换、装甲规格查询等基础功能。

#[repr(u8)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
/// 装甲类型枚举：小装甲或大装甲，对应不同的物理尺寸和碰撞体
pub enum ArmorType {
    /// 小装甲
    Small = 0,
    /// 大装甲
    Large = 1,
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
/// 装甲标签枚举，定义所有可用的装甲标识编号，对应 RoboMaster 竞赛规则中的装甲 ID
pub enum ArmorLabel {
    /// G号标签 - 工程机器人专用
    EngineerG = 0,
    /// 1号标签 - 英雄机器人专用
    HeroOne = 1,
    /// 2号标签 - 步兵机器人专用（3号机）
    InfantryTwo = 2,
    /// 3号标签 - 步兵机器人专用（4号机）
    InfantryOrHeroThree = 3,
    /// 4号标签 - 步兵机器人备用编号
    InfantryOrHeroFour = 4,
    /// O号标签 - 前哨站装甲模块
    OutpostZeo = 5,
    /// Bs号标签 - 基地小装甲模块
    BaseSmall = 6,
    /// Bb号标签 - 基地大装甲模块
    BaseLarge = 7,
    /// 5号标签 - 遗留/保留装甲编号
    HeroLegacyFive = 255,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
/// 小装甲标签枚举，定义可用的标准小装甲贴纸标识
pub enum SmallArmorLabel {
    /// 基地小装甲
    BaseSmall,
    /// 工程机器人 G 号装甲
    EngineerG,
    /// 前哨站装甲
    Outpost,
    /// 步兵机器人 2 号装甲
    InfantryTwo,
    /// 步兵/英雄机器人 3 号装甲
    InfantryOrHeroThree,
    /// 步兵/英雄机器人 4 号装甲
    InfantryOrHeroFour,
    /// 遗留 5 号装甲
    HeroLegacyFive,
}

impl SmallArmorLabel {
    /// 将小装甲标签转换为对应的通用装甲标签 ArmorLabel
    pub const fn label(self) -> ArmorLabel {
        match self {
            Self::BaseSmall => ArmorLabel::BaseSmall,
            Self::EngineerG => ArmorLabel::EngineerG,
            Self::Outpost => ArmorLabel::OutpostZeo,
            Self::InfantryTwo => ArmorLabel::InfantryTwo,
            Self::InfantryOrHeroThree => ArmorLabel::InfantryOrHeroThree,
            Self::InfantryOrHeroFour => ArmorLabel::InfantryOrHeroFour,
            Self::HeroLegacyFive => ArmorLabel::HeroLegacyFive,
        }
    }
}

/// 支持从小装甲标签直接转换为通用装甲标签
impl From<SmallArmorLabel> for ArmorLabel {
    fn from(label: SmallArmorLabel) -> Self {
        label.label()
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
/// 大装甲标签枚举，定义可用的标准大装甲贴纸标识
pub enum LargeArmorLabel {
    /// 英雄机器人 1 号装甲
    HeroOne,
    /// 步兵/英雄机器人 3 号装甲
    InfantryOrHeroThree,
    /// 步兵/英雄机器人 4 号装甲
    InfantryOrHeroFour,
    /// 遗留 5 号装甲
    HeroLegacyFive,
    /// 基地大装甲
    BaseLarge,
}

impl LargeArmorLabel {
    /// 将大装甲标签转换为对应的通用装甲标签 ArmorLabel
    pub const fn label(self) -> ArmorLabel {
        match self {
            Self::HeroOne => ArmorLabel::HeroOne,
            Self::InfantryOrHeroThree => ArmorLabel::InfantryOrHeroThree,
            Self::InfantryOrHeroFour => ArmorLabel::InfantryOrHeroFour,
            Self::HeroLegacyFive => ArmorLabel::HeroLegacyFive,
            Self::BaseLarge => ArmorLabel::BaseLarge,
        }
    }
}

/// 支持从大装甲标签直接转换为通用装甲标签
impl From<LargeArmorLabel> for ArmorLabel {
    fn from(label: LargeArmorLabel) -> Self {
        label.label()
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
/// 装甲规格枚举，同时携带装甲类型（小/大）和具体标签，用于唯一标识一块装甲
pub enum ArmorSpec {
    /// 小装甲及其标签
    Small(SmallArmorLabel),
    /// 大装甲及其标签
    Large(LargeArmorLabel),
}

impl ArmorSpec {
    /// 获取装甲类型（小装甲或大装甲）
    pub const fn armor_type(self) -> ArmorType {
        match self {
            Self::Small(_) => ArmorType::Small,
            Self::Large(_) => ArmorType::Large,
        }
    }

    /// 获取装甲对应的通用标签 ArmorLabel
    pub const fn label(self) -> ArmorLabel {
        match self {
            Self::Small(label) => label.label(),
            Self::Large(label) => label.label(),
        }
    }

    /// 获取该装甲规格对应的贴纸插槽列表，用于确定哪些贴纸实体属于该装甲
    pub const fn sticker_slots(self) -> &'static [ArmorStickerSlot] {
        match self {
            Self::Small(_) => &SMALL_ARMOR_STICKER_SLOTS,
            Self::Large(_) => &LARGE_ARMOR_STICKER_SLOTS,
        }
    }
}

/// 支持从小装甲标签转换为装甲规格
impl From<SmallArmorLabel> for ArmorSpec {
    fn from(label: SmallArmorLabel) -> Self {
        Self::Small(label)
    }
}

/// 支持从大装甲标签转换为装甲规格
impl From<LargeArmorLabel> for ArmorSpec {
    fn from(label: LargeArmorLabel) -> Self {
        Self::Large(label)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
/// 装甲贴纸插槽描述，包含标签和资源名称后缀，用于将贴纸实体与装甲规格关联
pub struct ArmorStickerSlot {
    /// 该插槽对应的装甲标签
    pub label: ArmorLabel,
    /// 贴纸资源名称后缀（如 "B", "G", "1", "2" 等），用于在场景中匹配贴纸实体
    pub name_suffix: &'static str,
}

/// 小装甲贴纸插槽列表，共 7 个标准插槽，按名称后缀 B/G/O/2/3/4/5 排列
pub const SMALL_ARMOR_STICKER_SLOTS: [ArmorStickerSlot; 7] = [
    ArmorStickerSlot {
        label: ArmorLabel::BaseSmall,
        name_suffix: "B",
    },
    ArmorStickerSlot {
        label: ArmorLabel::EngineerG,
        name_suffix: "G",
    },
    ArmorStickerSlot {
        label: ArmorLabel::OutpostZeo,
        name_suffix: "O",
    },
    ArmorStickerSlot {
        label: ArmorLabel::InfantryTwo,
        name_suffix: "2",
    },
    ArmorStickerSlot {
        label: ArmorLabel::InfantryOrHeroThree,
        name_suffix: "3",
    },
    ArmorStickerSlot {
        label: ArmorLabel::InfantryOrHeroFour,
        name_suffix: "4",
    },
    ArmorStickerSlot {
        label: ArmorLabel::HeroLegacyFive,
        name_suffix: "5",
    },
];

/// 大装甲贴纸插槽列表，共 5 个标准插槽，按名称后缀 1/3/4/5/B 排列
pub const LARGE_ARMOR_STICKER_SLOTS: [ArmorStickerSlot; 5] = [
    ArmorStickerSlot {
        label: ArmorLabel::HeroOne,
        name_suffix: "1",
    },
    ArmorStickerSlot {
        label: ArmorLabel::InfantryOrHeroThree,
        name_suffix: "3",
    },
    ArmorStickerSlot {
        label: ArmorLabel::InfantryOrHeroFour,
        name_suffix: "4",
    },
    ArmorStickerSlot {
        label: ArmorLabel::HeroLegacyFive,
        name_suffix: "5",
    },
    ArmorStickerSlot {
        label: ArmorLabel::BaseLarge,
        name_suffix: "B",
    },
];

impl ArmorLabel {
    /// 返回按传统顺序排列的装甲标签序列，用于调试循环切换贴纸
    pub fn sequence_small() -> &'static [ArmorLabel; 9] {
        &[
            ArmorLabel::EngineerG,
            ArmorLabel::HeroOne,
            ArmorLabel::InfantryTwo,
            ArmorLabel::InfantryOrHeroThree,
            ArmorLabel::InfantryOrHeroFour,
            ArmorLabel::OutpostZeo,
            ArmorLabel::BaseSmall,
            ArmorLabel::BaseLarge,
            ArmorLabel::HeroLegacyFive,
        ]
    }

    /// 根据装甲标签计算其在 sequence_small() 序列中的索引位置
    pub fn index_from_small(label: ArmorLabel) -> usize {
        match label {
            ArmorLabel::EngineerG => 0,
            ArmorLabel::HeroOne => 1,
            ArmorLabel::InfantryTwo => 2,
            ArmorLabel::InfantryOrHeroThree => 3,
            ArmorLabel::InfantryOrHeroFour => 4,
            ArmorLabel::OutpostZeo => 5,
            ArmorLabel::BaseSmall => 6,
            ArmorLabel::BaseLarge => 8,
            ArmorLabel::HeroLegacyFive => 7,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armor_spec_preserves_legacy_type_and_label() {
        let cases = [
            (
                ArmorSpec::Small(SmallArmorLabel::EngineerG),
                ArmorType::Small,
                ArmorLabel::EngineerG,
            ),
            (
                ArmorSpec::Small(SmallArmorLabel::Outpost),
                ArmorType::Small,
                ArmorLabel::OutpostZeo,
            ),
            (
                ArmorSpec::Large(LargeArmorLabel::HeroOne),
                ArmorType::Large,
                ArmorLabel::HeroOne,
            ),
            (
                ArmorSpec::Large(LargeArmorLabel::BaseLarge),
                ArmorType::Large,
                ArmorLabel::BaseLarge,
            ),
        ];

        for (spec, armor_type, label) in cases {
            assert_eq!(spec.armor_type(), armor_type);
            assert_eq!(spec.label(), label);
        }
    }

    #[test]
    fn debug_sequence_and_indexes_keep_legacy_order() {
        assert_eq!(
            ArmorLabel::sequence_small(),
            &[
                ArmorLabel::EngineerG,
                ArmorLabel::HeroOne,
                ArmorLabel::InfantryTwo,
                ArmorLabel::InfantryOrHeroThree,
                ArmorLabel::InfantryOrHeroFour,
                ArmorLabel::OutpostZeo,
                ArmorLabel::BaseSmall,
                ArmorLabel::BaseLarge,
                ArmorLabel::HeroLegacyFive,
            ]
        );

        assert_eq!(ArmorLabel::index_from_small(ArmorLabel::EngineerG), 0);
        assert_eq!(ArmorLabel::index_from_small(ArmorLabel::HeroOne), 1);
        assert_eq!(ArmorLabel::index_from_small(ArmorLabel::InfantryTwo), 2);
        assert_eq!(
            ArmorLabel::index_from_small(ArmorLabel::InfantryOrHeroThree),
            3
        );
        assert_eq!(
            ArmorLabel::index_from_small(ArmorLabel::InfantryOrHeroFour),
            4
        );
        assert_eq!(ArmorLabel::index_from_small(ArmorLabel::OutpostZeo), 5);
        assert_eq!(ArmorLabel::index_from_small(ArmorLabel::BaseSmall), 6);
        assert_eq!(ArmorLabel::index_from_small(ArmorLabel::HeroLegacyFive), 7);
        assert_eq!(ArmorLabel::index_from_small(ArmorLabel::BaseLarge), 8);
    }

    #[test]
    fn sticker_slot_tables_keep_asset_suffixes() {
        assert_eq!(
            ArmorSpec::Small(SmallArmorLabel::Outpost).sticker_slots(),
            &[
                ArmorStickerSlot {
                    label: ArmorLabel::BaseSmall,
                    name_suffix: "B",
                },
                ArmorStickerSlot {
                    label: ArmorLabel::EngineerG,
                    name_suffix: "G",
                },
                ArmorStickerSlot {
                    label: ArmorLabel::OutpostZeo,
                    name_suffix: "O",
                },
                ArmorStickerSlot {
                    label: ArmorLabel::InfantryTwo,
                    name_suffix: "2",
                },
                ArmorStickerSlot {
                    label: ArmorLabel::InfantryOrHeroThree,
                    name_suffix: "3",
                },
                ArmorStickerSlot {
                    label: ArmorLabel::InfantryOrHeroFour,
                    name_suffix: "4",
                },
                ArmorStickerSlot {
                    label: ArmorLabel::HeroLegacyFive,
                    name_suffix: "5",
                },
            ]
        );

        assert_eq!(
            ArmorSpec::Large(LargeArmorLabel::HeroOne).sticker_slots(),
            &[
                ArmorStickerSlot {
                    label: ArmorLabel::HeroOne,
                    name_suffix: "1",
                },
                ArmorStickerSlot {
                    label: ArmorLabel::InfantryOrHeroThree,
                    name_suffix: "3",
                },
                ArmorStickerSlot {
                    label: ArmorLabel::InfantryOrHeroFour,
                    name_suffix: "4",
                },
                ArmorStickerSlot {
                    label: ArmorLabel::HeroLegacyFive,
                    name_suffix: "5",
                },
                ArmorStickerSlot {
                    label: ArmorLabel::BaseLarge,
                    name_suffix: "B",
                },
            ]
        );
    }
}
