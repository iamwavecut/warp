use std::io::{self, Write as _};

use uuid::Uuid;
use warp_cli::agent::OutputFormat;
use warp_cli::schedule::{
    CreateScheduleArgs, ScheduleCadenceArgs, ScheduleCommand, ScheduleEventsArgs,
    UpdateScheduleArgs,
};
use warpui::{AppContext, platform::TerminationMode};

use crate::ai::local_named_agents::LocalNamedAgentRepository;
use crate::ai::local_scheduler::{
    LocalSchedule, LocalScheduleCadence, LocalScheduleRepository, LocalScheduleTimezone,
    MissedRunPolicy, NewLocalSchedule,
};

pub fn run(
    ctx: &mut AppContext,
    command: ScheduleCommand,
    output_format: OutputFormat,
) -> anyhow::Result<()> {
    let result = run_inner(command, output_format);
    match result {
        Ok(()) => {
            ctx.terminate_app(TerminationMode::ForceTerminate, None);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn run_inner(command: ScheduleCommand, output_format: OutputFormat) -> anyhow::Result<()> {
    let repository = LocalScheduleRepository::open_current_scope()?;
    match command {
        ScheduleCommand::Create(args) => {
            let schedule = create_schedule(&repository, args)?;
            write_schedule(&schedule, output_format)?;
        }
        ScheduleCommand::List => write_schedules(&repository.list()?, output_format)?,
        ScheduleCommand::Show(args) => {
            let id = parse_id(&args.schedule_id)?;
            let schedule = repository
                .get(id)?
                .ok_or_else(|| anyhow::anyhow!("local schedule {id} was not found"))?;
            write_schedule(&schedule, output_format)?;
        }
        ScheduleCommand::Update(args) => {
            let schedule = update_schedule(&repository, args)?;
            write_schedule(&schedule, output_format)?;
        }
        ScheduleCommand::Pause(args) => {
            let schedule =
                repository.set_enabled(parse_id(&args.schedule_id)?, args.revision, false)?;
            write_schedule(&schedule, output_format)?;
        }
        ScheduleCommand::Resume(args) => {
            let schedule =
                repository.set_enabled(parse_id(&args.schedule_id)?, args.revision, true)?;
            write_schedule(&schedule, output_format)?;
        }
        ScheduleCommand::Delete(args) => {
            let id = parse_id(&args.schedule_id)?;
            repository.delete(id, args.revision)?;
            write_message(
                &serde_json::json!({"deleted": id}),
                &format!("Deleted local schedule {id}"),
                output_format,
            )?;
        }
        ScheduleCommand::Run(args) => {
            let id = parse_id(&args.schedule_id)?;
            repository.request_run(id)?;
            write_message(
                &serde_json::json!({"schedule_id": id, "queued": true}),
                &format!("Queued local schedule {id}; it will start in the running WarpOss app."),
                output_format,
            )?;
        }
        ScheduleCommand::Cancel(args) => {
            let id = parse_id(&args.schedule_id)?;
            repository.request_cancel(id)?;
            write_message(
                &serde_json::json!({"schedule_id": id, "cancellation_requested": true}),
                &format!("Requested cancellation of local schedule {id}."),
                output_format,
            )?;
        }
        ScheduleCommand::Events(args) => write_events(&repository, args, output_format)?,
    }
    Ok(())
}

fn create_schedule(
    repository: &LocalScheduleRepository,
    args: CreateScheduleArgs,
) -> anyhow::Result<LocalSchedule> {
    let agent = LocalNamedAgentRepository::for_user().resolve(&args.agent)?;
    repository
        .create(NewLocalSchedule {
            name: args.name,
            agent_id: agent.id(),
            prompt: args.prompt,
            working_directory: args.working_directory,
            cadence: parse_cadence(&args.cadence, None)?,
            timezone: LocalScheduleTimezone::parse(&args.timezone)?,
            missed_policy: MissedRunPolicy::from(args.missed),
            notify: !args.no_notify,
        })
        .map_err(Into::into)
}

fn update_schedule(
    repository: &LocalScheduleRepository,
    args: UpdateScheduleArgs,
) -> anyhow::Result<LocalSchedule> {
    let id = parse_id(&args.schedule_id)?;
    let current = repository
        .get(id)?
        .ok_or_else(|| anyhow::anyhow!("local schedule {id} was not found"))?;
    let agent_id = match args.agent {
        Some(selector) => LocalNamedAgentRepository::for_user()
            .resolve(&selector)?
            .id(),
        None => current.agent_id,
    };
    let working_directory = if args.clear_working_directory {
        None
    } else {
        args.working_directory.or(current.working_directory)
    };
    let notify = if args.notify {
        true
    } else if args.no_notify {
        false
    } else {
        current.notify
    };
    let replacement = NewLocalSchedule {
        name: args.name.unwrap_or(current.name),
        agent_id,
        prompt: args.prompt.unwrap_or(current.prompt),
        working_directory,
        cadence: parse_cadence(&args.cadence, Some(current.cadence))?,
        timezone: args
            .timezone
            .as_deref()
            .map(LocalScheduleTimezone::parse)
            .transpose()?
            .unwrap_or(current.timezone),
        missed_policy: args
            .missed
            .map(MissedRunPolicy::from)
            .unwrap_or(current.missed_policy),
        notify,
    };
    repository
        .update(args.revision, replacement, id)
        .map_err(Into::into)
}

fn parse_cadence(
    args: &ScheduleCadenceArgs,
    fallback: Option<LocalScheduleCadence>,
) -> anyhow::Result<LocalScheduleCadence> {
    match (&args.every, &args.daily) {
        (Some(duration), None) => Ok(LocalScheduleCadence::every((*duration).into())?),
        (None, Some(value)) => Ok(LocalScheduleCadence::daily(value)?),
        (None, None) => fallback.ok_or_else(|| anyhow::anyhow!("a cadence is required")),
        (Some(_), Some(_)) => anyhow::bail!("--every and --daily are mutually exclusive"),
    }
}

fn write_events(
    repository: &LocalScheduleRepository,
    args: ScheduleEventsArgs,
    output_format: OutputFormat,
) -> anyhow::Result<()> {
    let id = parse_id(&args.schedule_id)?;
    let after = match args.consumer.as_deref() {
        Some(consumer) => args.after.max(repository.cursor(consumer)?),
        None => args.after,
    };
    let events = repository.events_after(id, after, args.limit)?;
    if let (Some(consumer), Some(last)) = (args.consumer.as_deref(), events.last()) {
        repository.advance_cursor(consumer, last.sequence)?;
    }
    match output_format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&events)?),
        OutputFormat::Ndjson => {
            for event in events {
                println!("{}", serde_json::to_string(&event)?);
            }
        }
        OutputFormat::Text | OutputFormat::Pretty => {
            if events.is_empty() {
                println!("No local schedule events.");
            } else {
                for event in events {
                    println!(
                        "{} {:?} run={} {}",
                        event.sequence,
                        event.kind,
                        event
                            .run_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "-".into()),
                        event.detail.replace('\n', " ")
                    );
                }
            }
        }
    }
    io::stdout().flush()?;
    Ok(())
}

fn write_schedule(schedule: &LocalSchedule, output_format: OutputFormat) -> anyhow::Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Ndjson => {
            println!("{}", serde_json::to_string(schedule)?);
        }
        OutputFormat::Text | OutputFormat::Pretty => {
            println!("{} ({})", schedule.name, schedule.id);
            println!("  agent: {}", schedule.agent_id);
            println!(
                "  cadence: {} [{}]",
                schedule.cadence.display(),
                schedule.timezone.display()
            );
            println!("  missed: {:?}", schedule.missed_policy);
            println!("  enabled: {}", schedule.enabled);
            println!("  next_run_at_ms: {}", schedule.next_run_at);
            println!("  revision: {}", schedule.revision);
            if let Some(run_id) = schedule.active_run_id {
                println!("  active_run: {run_id}");
            }
        }
    }
    io::stdout().flush()?;
    Ok(())
}

fn write_schedules(schedules: &[LocalSchedule], output_format: OutputFormat) -> anyhow::Result<()> {
    match output_format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(schedules)?),
        OutputFormat::Ndjson => {
            for schedule in schedules {
                println!("{}", serde_json::to_string(schedule)?);
            }
        }
        OutputFormat::Text | OutputFormat::Pretty => {
            if schedules.is_empty() {
                println!("No local schedules.");
            } else {
                for schedule in schedules {
                    println!(
                        "{}\t{}\t{}\t{}\trev={}",
                        schedule.id,
                        if schedule.enabled {
                            "enabled"
                        } else {
                            "paused"
                        },
                        schedule.cadence.display(),
                        schedule.name,
                        schedule.revision
                    );
                }
            }
        }
    }
    io::stdout().flush()?;
    Ok(())
}

fn write_message(
    json: &serde_json::Value,
    text: &str,
    output_format: OutputFormat,
) -> anyhow::Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Ndjson => println!("{}", serde_json::to_string(json)?),
        OutputFormat::Text | OutputFormat::Pretty => println!("{text}"),
    }
    io::stdout().flush()?;
    Ok(())
}

fn parse_id(value: &str) -> anyhow::Result<Uuid> {
    Uuid::parse_str(value)
        .map_err(|error| anyhow::anyhow!("invalid schedule id `{value}`: {error}"))
}
