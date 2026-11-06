mod live_eval;

use std::io::BufRead;
use std::path::Path;

use crate::{db, users};

pub use live_eval::run as run_live_eval;

const USAGE: &str = "использование:
  nixyon-hub serve
  nixyon-hub user add <почта>       пароль читается из stdin
  nixyon-hub user password <почта>  новый пароль из stdin, все сессии закрываются
  nixyon-hub user remove <почта>
  nixyon-hub user list
  nixyon-hub live-eval <почта> <запись> [...]
                                    прогон записей из /datasets/live через eval";

pub enum Command {
    Serve,
    User(UserCommand),
    LiveEval { email: String, records: Vec<String> },
}

pub enum UserCommand {
    Add(String),
    Password(String),
    Remove(String),
    List,
}

pub fn parse(args: &[String]) -> Result<Command, &'static str> {
    match args {
        [cmd] if cmd == "serve" => Ok(Command::Serve),
        [user, action, email] if user == "user" && action == "add" => {
            Ok(Command::User(UserCommand::Add(email.clone())))
        }
        [user, action, email] if user == "user" && action == "password" => {
            Ok(Command::User(UserCommand::Password(email.clone())))
        }
        [user, action, email] if user == "user" && action == "remove" => {
            Ok(Command::User(UserCommand::Remove(email.clone())))
        }
        [user, action] if user == "user" && action == "list" => {
            Ok(Command::User(UserCommand::List))
        }
        [cmd, email, records @ ..] if cmd == "live-eval" && !records.is_empty() => {
            Ok(Command::LiveEval {
                email: email.clone(),
                records: records.to_vec(),
            })
        }
        _ => Err(USAGE),
    }
}

pub fn run_user(
    db_path: &Path,
    command: UserCommand,
    input: &mut dyn BufRead,
) -> Result<String, Box<dyn std::error::Error>> {
    let conn = db::open(db_path)?;
    let read_password = |input: &mut dyn BufRead| -> Result<String, Box<dyn std::error::Error>> {
        let mut line = String::new();
        input.read_line(&mut line)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    };
    match command {
        UserCommand::Add(email) => {
            let password = read_password(input)?;
            let user = users::add(&conn, &email, &password)?;
            Ok(format!("добавлен {}", user.email))
        }
        UserCommand::Password(email) => {
            let password = read_password(input)?;
            users::set_password(&conn, &email, &password)?;
            Ok("пароль сменён, сессии закрыты".into())
        }
        UserCommand::Remove(email) => {
            users::remove(&conn, &email)?;
            Ok("удалён".into())
        }
        UserCommand::List => Ok(users::list(&conn)?.join("\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn commands_are_parsed_and_unknown_shows_usage() {
        assert!(matches!(parse(&args(&["serve"])), Ok(Command::Serve)));
        assert!(matches!(
            parse(&args(&["user", "add", "a@x.org"])),
            Ok(Command::User(UserCommand::Add(_)))
        ));
        assert!(matches!(
            parse(&args(&["user", "list"])),
            Ok(Command::User(UserCommand::List))
        ));
        assert!(parse(&args(&["user", "add"])).is_err());
        assert!(matches!(
            parse(&args(&["live-eval", "a@x.org", "55342", "dialogue"])),
            Ok(Command::LiveEval { email, records }) if email == "a@x.org" && records == ["55342", "dialogue"]
        ));
        assert!(parse(&args(&["live-eval", "a@x.org"])).is_err());
        assert!(parse(&args(&[])).is_err());
    }

    #[test]
    fn user_is_added_with_password_from_input_not_arguments() {
        let path = std::env::temp_dir().join(format!("nixyon-cli-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut input = "очень-длинный-пароль\n".as_bytes();
        run_user(&path, UserCommand::Add("A@x.org".into()), &mut input).unwrap();
        let list = run_user(&path, UserCommand::List, &mut "".as_bytes()).unwrap();
        assert_eq!(list, "a@x.org");
        let mut short = "коротко\n".as_bytes();
        assert!(run_user(&path, UserCommand::Add("b@x.org".into()), &mut short).is_err());
        std::fs::remove_file(&path).unwrap();
    }
}
