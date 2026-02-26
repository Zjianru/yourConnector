//! 会话队列抽象：为不同事件类型提供统一排队语义。

use std::collections::{HashMap, VecDeque};

/// 队列键：用于给不同链路绑定不同排队策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum QueueKey {
    ToolDetails,
    ToolsRefresh,
    Metrics,
    PairingBanner,
    Control,
    Chat,
    Report,
}

/// 排队语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueSemantics {
    /// 同键只保留最后一个请求。
    LatestWins,
    /// 串行执行（不并发），保留顺序。
    Serialized,
    /// 先进先出。
    Fifo,
}

/// 单键队列策略。
#[derive(Debug, Clone, Copy)]
pub(crate) struct QueuePolicy {
    pub(crate) semantics: QueueSemantics,
    pub(crate) max_pending: usize,
}

impl QueuePolicy {
    /// Latest-wins 策略。
    pub(crate) const fn latest_wins() -> Self {
        Self {
            semantics: QueueSemantics::LatestWins,
            max_pending: 1,
        }
    }

    /// 串行策略。
    pub(crate) const fn serialized(max_pending: usize) -> Self {
        Self {
            semantics: QueueSemantics::Serialized,
            max_pending,
        }
    }

    /// FIFO 策略。
    pub(crate) const fn fifo(max_pending: usize) -> Self {
        Self {
            semantics: QueueSemantics::Fifo,
            max_pending,
        }
    }
}

/// 入队结果。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct QueueEnqueueReport {
    pub(crate) dropped: u32,
}

/// 通用队列调度器。
#[derive(Debug)]
pub(crate) struct QueueScheduler<T> {
    default_policy: QueuePolicy,
    policies: HashMap<QueueKey, QueuePolicy>,
    latest: HashMap<QueueKey, T>,
    latest_order: VecDeque<QueueKey>,
    fifo: VecDeque<(QueueKey, T)>,
    fifo_depth_by_key: HashMap<QueueKey, usize>,
}

impl<T> QueueScheduler<T> {
    /// 构造调度器。
    pub(crate) fn new(
        default_policy: QueuePolicy,
        policies: HashMap<QueueKey, QueuePolicy>,
    ) -> Self {
        Self {
            default_policy,
            policies,
            latest: HashMap::new(),
            latest_order: VecDeque::new(),
            fifo: VecDeque::new(),
            fifo_depth_by_key: HashMap::new(),
        }
    }

    /// 入队。
    pub(crate) fn enqueue(&mut self, key: QueueKey, item: T) -> QueueEnqueueReport {
        let policy = self.policy_for(key);
        let mut dropped = 0u32;
        match policy.semantics {
            QueueSemantics::LatestWins => {
                if self.latest.contains_key(&key) {
                    dropped = 1;
                } else {
                    self.latest_order.push_back(key);
                }
                self.latest.insert(key, item);
            }
            QueueSemantics::Serialized | QueueSemantics::Fifo => {
                let depth = self.depth_for_key(key);
                if policy.max_pending > 0 && depth >= policy.max_pending {
                    dropped = 1;
                } else {
                    self.fifo.push_back((key, item));
                    *self.fifo_depth_by_key.entry(key).or_insert(0) += 1;
                }
            }
        }
        QueueEnqueueReport { dropped }
    }

    /// 弹出下一个待处理项。
    pub(crate) fn pop_next(&mut self) -> Option<(QueueKey, T)> {
        if let Some((key, item)) = self.fifo.pop_front() {
            if let Some(depth) = self.fifo_depth_by_key.get_mut(&key) {
                *depth = depth.saturating_sub(1);
                if *depth == 0 {
                    self.fifo_depth_by_key.remove(&key);
                }
            }
            return Some((key, item));
        }

        while let Some(key) = self.latest_order.pop_front() {
            if let Some(item) = self.latest.remove(&key) {
                return Some((key, item));
            }
        }
        None
    }

    /// 读取 latest-wins 槽位中的可变引用。
    pub(crate) fn latest_mut(&mut self, key: QueueKey) -> Option<&mut T> {
        self.latest.get_mut(&key)
    }

    /// 单键深度。
    pub(crate) fn depth_for_key(&self, key: QueueKey) -> usize {
        let latest_depth = usize::from(self.latest.contains_key(&key));
        latest_depth
            + self
                .fifo_depth_by_key
                .get(&key)
                .copied()
                .unwrap_or_default()
    }
    /// 读取指定键的策略（不存在时返回默认策略）。
    fn policy_for(&self, key: QueueKey) -> QueuePolicy {
        self.policies
            .get(&key)
            .copied()
            .unwrap_or(self.default_policy)
    }
}
