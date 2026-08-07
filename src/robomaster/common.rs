// 引入装甲规格、装甲标签等上层装甲相关类型
use crate::robomaster::prelude::*;

/// 阵营枚举：红方 / 蓝方
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum Team {
    Red,   // 红方阵营
    Blue,  // 蓝方阵营
}

impl Team {
    /// 根据字符串解析阵营名称（大小写不敏感）
    /// 传入 "red"/"RED" 返回 Some(Red)，"blue" 返回 Some(Blue)，其他字符串返回 None
    pub fn from(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "red" => Some(Team::Red),
            "blue" => Some(Team::Blue),
            _ => None,
        }
    }
}

/// 机器人全局配置：只保存装甲相关配置
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct RobotConfig {
    /// 装甲规格：大装甲 / 小装甲 + 装甲编号标签
    pub armor: ArmorSpec,
    /// 该机器人身上装甲片总数量（RM规则绝大多数机器人都是4块装甲）
    pub armor_count: usize,
}

impl RobotConfig {
    /// 常量构造函数（const fn），编译期即可构造实例，用来生成全局常量配置
    pub const fn new(armor: ArmorSpec, armor_count: usize) -> Self {
        Self { armor, armor_count }
    }
}

// ===================== 全局常量：各机型出厂装甲配置 =====================
/// 英雄机器人配置：大块装甲、Hero一号装甲标识、4片装甲
pub const HERO_ROBOT_CONFIG: RobotConfig =
    RobotConfig::new(ArmorSpec::Large(LargeArmorLabel::HeroOne), 4);

/// 工程机器人配置：小型装甲、工程专属装甲标签、4片装甲
pub const ENGINEER_ROBOT_CONFIG: RobotConfig =
    RobotConfig::new(ArmorSpec::Small(SmallArmorLabel::EngineerG), 4);

/// 3号步兵配置：小型装甲、步兵3装甲标签、4装甲
pub const INFANTRY_THREE_CONFIG: RobotConfig =
    RobotConfig::new(ArmorSpec::Small(SmallArmorLabel::InfantryOrHeroThree), 4);

/// 4号步兵配置：小型装甲、步兵4装甲标签、4装甲
pub const INFANTRY_FOUR_CONFIG: RobotConfig =
    RobotConfig::new(ArmorSpec::Small(SmallArmorLabel::InfantryOrHeroFour), 4);

/// 哨兵二号配置
pub const SENTINEL_ROBOT_TWO_CONFIG: RobotConfig =
    RobotConfig::new(ArmorSpec::Small(SmallArmorLabel::InfantryTwo), 4);

/// 哨兵三号配置
pub const SENTINEL_ROBOT_THREE_CONFIG: RobotConfig =
    RobotConfig::new(ArmorSpec::Small(SmallArmorLabel::InfantryOrHeroThree), 4);

/// 哨兵四号配置
pub const SENTINEL_ROBOT_FOUR_CONFIG: RobotConfig =
    RobotConfig::new(ArmorSpec::Small(SmallArmorLabel::InfantryOrHeroFour), 4);

/// RM 全部机器人机型枚举，完整对应官方赛场所有兵种
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum Robot {
    /// 英雄机器人 - 1号机
    /// 唯一搭载42mm大口径发射机构，血量最高、伤害高，具备部署形态
    Hero,

    /// 工程机器人 - 2号机
    /// 无发射炮管，核心职责拾取能量机关、救援友方机器人、团队增益
    Engineer,

    /// 步兵机器人 - 3、4号步兵
    /// 基础作战单位，发射17mm弹丸，性能均衡，可通过击杀获取经验升级
    Infantry,

    /// 空中无人机（空中机器人）- 6号机
    /// 具备飞行能力，搭载第一视角图传、激光反制模块，17mm子弹打击地面单位
    Aerial,

    /// 哨兵机器人 - 7号机，基地防守专用
    /// 支持全自动自主巡逻防御、姿态切换、占领己方堡垒
    Sentinel,

    /// 飞镖发射系统 - 8号
    /// 远程投射飞镖，专门用来攻击敌方前哨站、基地核心
    DartSystem,

    /// 雷达站 - 9号
    /// 战场探测、激光照射标记敌方、解析敌方信息波进行反制
    Radar,
}

// ===================== 单元测试模块 =====================
#[cfg(test)]
mod tests {
    use super::*;

    /// 测试目标：保证各个机器人常量配置不会在重构时被误改
    /// 校验每个机型对应的【装甲大小、装甲标签、装甲数量】和历史规则保持一致
    #[test]
    fn robot_configs_preserve_legacy_armor_values() {
        // 测试用例数组：(机器人配置, 预期装甲类型, 预期装甲标签, 预期装甲数量)
        let cases = [
            (HERO_ROBOT_CONFIG, ArmorType::Large, ArmorLabel::HeroOne, 4),
            (
                ENGINEER_ROBOT_CONFIG,
                ArmorType::Small,
                ArmorLabel::EngineerG,
                4,
            ),
            (
                INFANTRY_THREE_CONFIG,
                ArmorType::Small,
                ArmorLabel::InfantryOrHeroThree,
                4,
            ),
            (
                INFANTRY_FOUR_CONFIG,
                ArmorType::Small,
                ArmorLabel::InfantryOrHeroFour,
                4,
            ),
            (
                SENTINEL_ROBOT_TWO_CONFIG,
                ArmorType::Small,
                ArmorLabel::InfantryTwo,
                4,
            ),
            (
                SENTINEL_ROBOT_THREE_CONFIG,
                ArmorType::Small,
                ArmorLabel::InfantryOrHeroThree,
                4,
            ),
            (
                SENTINEL_ROBOT_FOUR_CONFIG,
                ArmorType::Small,
                ArmorLabel::InfantryOrHeroFour,
                4,
            ),
        ];

        // 遍历校验每一条配置
        for (config, armor_type, label, armor_count) in cases {
            // 校验装甲是大装甲还是小装甲
            assert_eq!(config.armor.armor_type(), armor_type);
            // 校验装甲对应的编号标签
            assert_eq!(config.armor.label(), label);
            // 校验装甲总数固定为4
            assert_eq!(config.armor_count, armor_count);
        }
    }
}