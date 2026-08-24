use std::time::Duration;

use diesel::RunQueryDsl as _;
use diesel::sql_query;
use tempfile::tempdir;
use uuid::Uuid;

use super::{
    LocalScheduleCadence, LocalScheduleEventKind, LocalScheduleRepository, LocalScheduleTimezone,
    MissedRunPolicy, NewLocalSchedule,
};

fn input(policy: MissedRunPolicy) -> NewLocalSchedule {
    NewLocalSchedule {
        name: "Daily review".into(),
        agent_id: Uuid::new_v4(),
        prompt: "Review the current local changes".into(),
        working_directory: None,
        cadence: LocalScheduleCadence::every(Duration::from_secs(60)).unwrap(),
        timezone: LocalScheduleTimezone::Utc,
        missed_policy: policy,
        notify: true,
    }
}

fn force_due(repository: &LocalScheduleRepository, id: Uuid, timestamp: i64) {
    repository
        .with_connection(|connection| {
            sql_query(format!(
                "UPDATE local_schedules SET next_run_at = {timestamp} WHERE id = '{id}'"
            ))
            .execute(connection)
            .unwrap();
            Ok(())
        })
        .unwrap();
}

#[test]
fn p2_4_local_schedule_crud_journal_and_cursor_survive_restart() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("scheduler.sqlite");
    let repository = LocalScheduleRepository::open(&database).unwrap();
    let created = repository.create(input(MissedRunPolicy::RunOnce)).unwrap();

    let replacement = NewLocalSchedule {
        name: "Updated review".into(),
        notify: false,
        ..input(MissedRunPolicy::CatchUp)
    };
    let updated = repository
        .update(created.revision, replacement, created.id)
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.name, "Updated review");
    assert!(!updated.notify);

    repository.request_run(updated.id).unwrap();
    let claim = repository
        .claim_due(super::now_millis())
        .unwrap()
        .pop()
        .unwrap();
    assert!(claim.manual);
    assert!(
        repository
            .finish_run(
                updated.id,
                claim.run_id,
                LocalScheduleEventKind::Completed,
                "done"
            )
            .unwrap()
    );
    let events = repository.events_after(updated.id, 0, 20).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, LocalScheduleEventKind::Started);
    assert_eq!(events[1].kind, LocalScheduleEventKind::Completed);
    assert_eq!(
        repository
            .advance_cursor("test-consumer", events[1].sequence)
            .unwrap(),
        2
    );
    drop(repository);

    let restarted = LocalScheduleRepository::open(&database).unwrap();
    assert_eq!(
        restarted.get(updated.id).unwrap().unwrap().name,
        "Updated review"
    );
    assert_eq!(restarted.cursor("test-consumer").unwrap(), 2);
    assert_eq!(restarted.events_after(updated.id, 0, 20).unwrap(), events);
}

#[test]
fn p2_4_missed_run_policies_skip_coalesce_or_catch_up_locally() {
    let now = super::now_millis();

    let skipped_repo = LocalScheduleRepository::in_memory().unwrap();
    let skipped = skipped_repo.create(input(MissedRunPolicy::Skip)).unwrap();
    force_due(&skipped_repo, skipped.id, now - 180_000);
    assert!(skipped_repo.claim_due(now).unwrap().is_empty());
    let skipped_events = skipped_repo.events_after(skipped.id, 0, 10).unwrap();
    assert_eq!(skipped_events[0].kind, LocalScheduleEventKind::Missed);
    assert!(skipped_repo.get(skipped.id).unwrap().unwrap().next_run_at > now);

    let once_repo = LocalScheduleRepository::in_memory().unwrap();
    let once = once_repo.create(input(MissedRunPolicy::RunOnce)).unwrap();
    force_due(&once_repo, once.id, now - 180_000);
    let once_claim = once_repo.claim_due(now).unwrap().pop().unwrap();
    assert!(!once_claim.manual);
    assert!(once_repo.get(once.id).unwrap().unwrap().next_run_at > now);

    let catch_up_repo = LocalScheduleRepository::in_memory().unwrap();
    let catch_up = catch_up_repo
        .create(input(MissedRunPolicy::CatchUp))
        .unwrap();
    force_due(&catch_up_repo, catch_up.id, now - 180_000);
    let catch_up_claim = catch_up_repo.claim_due(now).unwrap().pop().unwrap();
    let claimed = catch_up_repo.get(catch_up.id).unwrap().unwrap();
    assert!(claimed.next_run_at <= now);
    catch_up_repo
        .finish_run(
            catch_up.id,
            catch_up_claim.run_id,
            LocalScheduleEventKind::Completed,
            "caught up once",
        )
        .unwrap();
    assert_eq!(catch_up_repo.claim_due(now).unwrap().len(), 1);
}

#[test]
fn p2_4_active_run_is_recovered_as_interrupted_and_can_be_cancelled() {
    let repository = LocalScheduleRepository::in_memory().unwrap();
    let schedule = repository.create(input(MissedRunPolicy::RunOnce)).unwrap();
    repository.request_run(schedule.id).unwrap();
    let claim = repository
        .claim_due(super::now_millis())
        .unwrap()
        .pop()
        .unwrap();
    repository.request_cancel(schedule.id).unwrap();
    assert_eq!(
        repository.cancellation_requests().unwrap(),
        vec![(schedule.id, claim.run_id)]
    );
    assert_eq!(repository.recover_interrupted_runs().unwrap(), 1);
    assert!(
        repository
            .get(schedule.id)
            .unwrap()
            .unwrap()
            .active_run_id
            .is_none()
    );
    assert_eq!(
        repository
            .events_after(schedule.id, 0, 10)
            .unwrap()
            .last()
            .unwrap()
            .kind,
        LocalScheduleEventKind::Interrupted
    );
}

#[test]
fn p2_4_daily_cadence_validates_timezone_and_resolves_future_wall_time() {
    assert!(LocalScheduleCadence::daily("9:00").is_err());
    assert!(LocalScheduleCadence::daily("24:00").is_err());
    let cadence = LocalScheduleCadence::daily("09:30").unwrap();
    let timezone = LocalScheduleTimezone::parse("+02:30").unwrap();
    assert_eq!(timezone.display(), "+02:30");
    let now = super::now_millis();
    let next = cadence.next_after(now, &timezone).unwrap();
    assert!(next > now);
    assert!(next - now <= 25 * 60 * 60 * 1_000);
    assert!(LocalScheduleTimezone::parse("Europe/Warsaw").is_err());
}
