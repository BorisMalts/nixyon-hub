use nixyon_agent::protocol::{JobState, Op, ServerFrame};

use crate::agent_link::{AgentLink, Job, LinkError};
use crate::config::HubConfig;
use crate::events::{self, Event, EventKind};
use crate::{clock, db, users};

/// Запускает прогон живых записей от имени пользователя хаба, как кнопка на странице:
/// записи сверяет агент, запуск попадает в журнал событий. Нужен доступ к контейнеру
/// хаба, поэтому вход и Origin не проверяются.
pub async fn run(
    config: &HubConfig,
    email: &str,
    records: Vec<String>,
) -> Result<String, Box<dyn std::error::Error>> {
    let conn = db::open(&config.db_path)?;
    let user = users::find(&conn, email)?.ok_or("нет такого пользователя хаба")?;
    let link = AgentLink::new(config.agent_addr.clone(), config.agent_token.clone());
    let job = Job {
        records,
        ..Job::default()
    };
    let frame = match link.single_with(Op::LiveEval, &user.email, job).await {
        Ok(frame) => frame,
        Err(LinkError::Agent(_, message)) => return Err(message.into()),
        Err(other) => return Err(other.into()),
    };
    let ServerFrame::Started { job } = frame else {
        return Err("агент ответил непонятно".into());
    };
    events::record(
        &conn,
        &Event {
            kind: EventKind::LiveEvalStarted,
            user_id: Some(user.id),
            email: Some(&user.email),
            ip: "cli",
            user_agent: "nixyon-hub live-eval",
            detail: &job.id.to_string(),
        },
        clock::now(),
    )?;
    let state = match job.state {
        JobState::Queued => "ждёт в очереди",
        JobState::Running => "запущено",
        JobState::Succeeded | JobState::Failed => "уже завершено",
    };
    Ok(format!("задание {}: {state}", job.id))
}
