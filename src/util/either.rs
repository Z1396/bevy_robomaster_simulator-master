/// 二选一枚举：要么持有左类型 L，要么持有右类型 R
/// 场景：二选一迭代器、分支返回值、两种不同来源的数据流
pub enum Either<L, R> {
    /// 左侧变体
    Left(L),
    /// 右侧变体
    Right(R),
}

/// 为 Either 实现 Iterator trait
/// 约束条件：L 和 R 都必须是迭代器，且两者产出的元素类型完全一致 T
impl<L, R, T> Iterator for Either<L, R>
where
    L: Iterator<Item = T>,
    R: Iterator<Item = T>,
{
    /// 迭代器产出元素类型为 T
    type Item = T;

    /// 迭代取下一个元素
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            // 当前是 Left 变体，则代理调用左侧迭代器的 next()
            Either::Left(l) => l.next(),
            // 当前是 Right 变体，则代理调用右侧迭代器的 next()
            Either::Right(r) => r.next(),
        }
    }

    /// 实现 size_hint，向调用者返回迭代器剩余元素的下界、上界
    /// 依然直接代理内部迭代器的 size_hint，保证迭代器接口完整性
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Either::Left(l) => l.size_hint(),
            Either::Right(r) => r.size_hint(),
        }
    }
}