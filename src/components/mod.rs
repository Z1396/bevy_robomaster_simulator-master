// 声明当前模块下存在 3 个子模块，分别对应三个独立的 rs 文件：
// camera.rs、infantry.rs、physics.rs
mod camera;
mod infantry;
mod physics;

// 对外重新导出 camera 模块内所有公开成员（pub 修饰的结构体、函数、常量、Trait等）
pub use camera::*;
// 对外重新导出 infantry 步兵模块所有公开内容
pub use infantry::*;
// 对外重新导出 physics 物理模块所有公开内容
pub use physics::*;