/// 全局导出宏：快速定义结构体，并自动生成全字段入参的 new 构造函数
/// 支持普通 struct / pub struct 两种写法
#[macro_export]
macro_rules! all_arg_constructor {
    // 分支1：定义 私有结构体 struct Name { ... }
    (struct $name:ident { $( $field:ident : $ty:ty ),* $(,)? }) => {
        // 原样生成结构体
        struct $name {
            $(
                $field: $ty,
            )*
        }
        // 为结构体实现 new 构造方法：所有字段依次作为入参，直接构造 Self
        impl $name {
            /// 全字段构造器，每个结构体字段都需要传参
            pub fn new($( $field: $ty ),*) -> Self {
                Self { $( $field ),* }
            }
        }
    };

    // 分支2：定义 公开结构体 pub struct Name { ... }
    (pub struct $name:ident { $( $field:ident : $ty:ty ),* $(,)? }) => {
        pub struct $name {
            $(
                $field: $ty,
            )*
        }
        impl $name {
            pub fn new($( $field: $ty ),*) -> Self {
                Self { $( $field ),* }
            }
        }
    };
}