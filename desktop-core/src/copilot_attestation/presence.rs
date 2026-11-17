//! Bounded convergence with normal registration lifecycle policy.
use std::time::{Duration, Instant};

pub(super) async fn wait_for_active<T, E>(
    mut read: impl FnMut() -> Result<Option<T>, E>,
    deadline: Instant,
) -> Result<Option<T>, E> {
    loop {
        let Some(remaining) = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
        else {
            return Ok(None);
        };
        if let Some(value) = read()? {
            return Ok(Some(value));
        }
        tokio::time::sleep(remaining.min(Duration::from_millis(25))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_existing_inactive_registration_can_become_active() {
        let mut reads = 0;
        let value = wait_for_active(
            || {
                reads += 1;
                Ok::<_, ()>((reads == 2).then_some("same existing registration"))
            },
            Instant::now() + Duration::from_secs(1),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(value, "same existing registration");
        assert_eq!(reads, 2);
    }

    #[tokio::test]
    async fn removal_or_owner_change_ends_the_wait_immediately() {
        let mut reads = 0;
        let result = wait_for_active::<(), ()>(
            || {
                reads += 1;
                if reads == 1 {
                    Ok(None)
                } else {
                    Err(())
                }
            },
            Instant::now() + Duration::from_secs(1),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(reads, 2);
    }

    #[tokio::test]
    async fn inactive_registrations_remain_refused_at_the_deadline() {
        assert!(
            wait_for_active::<(), ()>(|| Ok(None), Instant::now() + Duration::from_millis(10))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn an_expired_wait_does_not_read_or_accept_even_ready_state() {
        assert!(
            wait_for_active::<(), ()>(|| panic!("expired wait read state"), Instant::now())
                .await
                .unwrap()
                .is_none()
        );
    }
}
