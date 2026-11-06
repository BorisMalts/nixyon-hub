use serde::{Deserialize, Serialize};

use super::{DatasetSummary, JobStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    BadRequest,
    Busy,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    Started {
        job: JobStatus,
    },
    Status {
        job: Option<JobStatus>,
        #[serde(default)]
        queue: Vec<JobStatus>,
    },
    Log {
        line: String,
    },
    Finished {
        job: JobStatus,
    },
    Output {
        data: Vec<u8>,
    },
    Exit {
        code: Option<u32>,
    },
    Datasets {
        datasets: Vec<DatasetSummary>,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}
