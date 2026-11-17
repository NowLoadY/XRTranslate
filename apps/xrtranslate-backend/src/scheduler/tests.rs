use super::*;
use InferenceWorkload::{Offline, Realtime};
use futures_util::poll;

#[tokio::test]
async fn parallel_work_keeps_per_model_limits_without_head_of_line_blocking() {
    let scheduler = InferenceScheduler::new(1, 2);
    let asr = scheduler.acquire_asr(Realtime).await;
    let mut waiting_asr = Box::pin(scheduler.acquire_asr(Realtime));
    assert!(poll!(&mut waiting_asr).is_pending());
    let first = scheduler.acquire_translation(Realtime).await;
    let second = scheduler.acquire_translation(Realtime).await;
    let mut third = Box::pin(scheduler.acquire_translation(Realtime));
    assert!(poll!(&mut third).is_pending());
    drop(first);
    drop(third.await);
    assert!(poll!(&mut waiting_asr).is_pending());
    drop(asr);
    drop(waiting_asr.await);
    drop(second);
}

#[tokio::test]
async fn switching_to_sequential_drains_active_work_and_respects_waiting_order() {
    let scheduler = InferenceScheduler::new(2, 2);
    let asr = scheduler.acquire_asr(Realtime).await;
    let translation = scheduler.acquire_translation(Realtime).await;
    scheduler.set_parallel(false);
    let mut next_asr = Box::pin(scheduler.acquire_asr(Realtime));
    let mut next_translation = Box::pin(scheduler.acquire_translation(Realtime));
    assert!(poll!(&mut next_asr).is_pending());
    assert!(poll!(&mut next_translation).is_pending());
    drop(asr);
    assert!(poll!(&mut next_asr).is_pending());
    drop(translation);
    let asr = next_asr.await;
    assert!(poll!(&mut next_translation).is_pending());
    scheduler.set_parallel(true);
    drop(next_translation.await);
    drop(asr);
}

#[tokio::test]
async fn cancelled_waiters_return_capacity_before_and_after_admission() {
    let scheduler = InferenceScheduler::new(1, 1);
    scheduler.set_parallel(false);
    let active = scheduler.acquire_asr(Realtime).await;
    let mut cancelled = Box::pin(scheduler.acquire_translation(Realtime));
    let mut admitted = Box::pin(scheduler.acquire_asr(Realtime));
    let mut next = Box::pin(scheduler.acquire_translation(Realtime));
    assert!(poll!(&mut cancelled).is_pending());
    assert!(poll!(&mut admitted).is_pending());
    assert!(poll!(&mut next).is_pending());
    drop(cancelled);
    drop(active);
    // The permit has been delivered, but its future has not been polled again.
    drop(admitted);
    drop(next.await);
    drop(scheduler.acquire_asr(Realtime).await);
}

#[tokio::test]
async fn realtime_priority_does_not_starve_offline_work() {
    let scheduler = InferenceScheduler::new(1, 1);
    scheduler.set_parallel(false);
    let active = scheduler.acquire_asr(Realtime).await;
    let mut offline = Box::pin(scheduler.acquire_translation(Offline));
    assert!(poll!(&mut offline).is_pending());
    let mut realtime = (0..REALTIME_BURST_LIMIT * 2)
        .map(|_| Box::pin(scheduler.acquire_asr(Realtime)))
        .collect::<VecDeque<_>>();
    for request in &mut realtime {
        assert!(poll!(request).is_pending());
    }
    drop(active);
    for _ in 1..REALTIME_BURST_LIMIT {
        assert!(poll!(&mut offline).is_pending());
        drop(realtime.pop_front().unwrap().await);
    }
    drop(offline.await);
    for request in realtime {
        drop(request.await);
    }
}
