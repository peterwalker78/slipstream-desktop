//! Made-up hook payloads and agent records, shaped as each agent sends and keeps them.
//! `@ROOT@` is the scratch machine's root.

pub mod claude {
    pub const SESSION: &str = "5c1d7e2a-9b3f-4c8e-a1d2-3e4f5a6b7c8d";

    pub const SESSION_START: &str = r#"{"session_id":"5c1d7e2a-9b3f-4c8e-a1d2-3e4f5a6b7c8d","transcript_path":"@ROOT@/home/.claude/projects/-home-robin-projects-kiln/5c1d7e2a.jsonl","cwd":"/home/robin/projects/kiln","hook_event_name":"SessionStart","source":"startup"}"#;

    pub const PROMPT: &str = r#"{"session_id":"5c1d7e2a-9b3f-4c8e-a1d2-3e4f5a6b7c8d","transcript_path":"@ROOT@/home/.claude/projects/-home-robin-projects-kiln/5c1d7e2a.jsonl","cwd":"/home/robin/projects/kiln","permission_mode":"default","hook_event_name":"UserPromptSubmit","prompt":"Make the kiln timer count down from the firing schedule\ninstead of up from zero, and show it in the header\n\n"}"#;

    pub const TOOL_USED: &str = r#"{"session_id":"5c1d7e2a-9b3f-4c8e-a1d2-3e4f5a6b7c8d","transcript_path":"@ROOT@/home/.claude/projects/-home-robin-projects-kiln/5c1d7e2a.jsonl","cwd":"/home/robin/projects/kiln","hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{"file_path":"/home/robin/projects/kiln/src/timer.rs"},"tool_response":{"success":true}}"#;

    pub const SUBAGENT_TOOL_USED: &str = r#"{"session_id":"5c1d7e2a-9b3f-4c8e-a1d2-3e4f5a6b7c8d","transcript_path":"@ROOT@/home/.claude/projects/-home-robin-projects-kiln/agent-b81.jsonl","cwd":"/home/robin/projects/kiln","hook_event_name":"PostToolUse","agent_id":"b81f","agent_type":"Explore","prompt":"Find every caller of the timer","tool_name":"Grep","tool_input":{"pattern":"Timer::new"}}"#;

    pub const STOP: &str = r#"{"session_id":"5c1d7e2a-9b3f-4c8e-a1d2-3e4f5a6b7c8d","transcript_path":"@ROOT@/home/.claude/projects/-home-robin-projects-kiln/5c1d7e2a.jsonl","cwd":"/home/robin/projects/kiln","hook_event_name":"Stop","stop_hook_active":false,"last_assistant_message":"The timer now counts down.\n\n<b>Tests</b> pass & the header shows the time left."}"#;

    pub const PERMISSION: &str = r#"{"session_id":"5c1d7e2a-9b3f-4c8e-a1d2-3e4f5a6b7c8d","transcript_path":"@ROOT@/home/.claude/projects/-home-robin-projects-kiln/5c1d7e2a.jsonl","cwd":"/home/robin/projects/kiln","hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"}"#;

    pub const TRANSCRIPT_PATH: &str =
        "home/.claude/projects/-home-robin-projects-kiln/5c1d7e2a.jsonl";

    pub const TRANSCRIPT: &str = r#"{"type":"user","message":{"role":"user","content":"Make the kiln timer count down"},"uuid":"u1"}
{"type":"ai-title","aiTitle":"Kiln timer countdown","sessionId":"5c1d7e2a"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Looking at the timer."}]},"uuid":"a1"}
{"type": "custom-title", "customTitle": "Kiln timer: count down\tfrom the schedule", "sessionId": "5c1d7e2a"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done."}]},"uuid":"a2"}
"#;

    /// The entry Claude Code keeps for a session running in the background as AGENT_PID.
    pub const SESSION_ENTRY: &str = r#"{"pid":4242,"sessionId":"5c1d7e2a-9b3f-4c8e-a1d2-3e4f5a6b7c8d","cwd":"/home/robin/projects/kiln","jobId":"5c1d7e2a"}"#;

    pub const JOB_STATE: &str = r#"{"name":"Kiln timer\nrework","state":"working","fan":[
        {"kind":"agent","label":"Reading the firing schedule","startedAt":1},
        {"kind":"agent","label":"Already done","doneAt":5},
        {"kind":"bash","label":"cargo test"},
        {"kind":"agent","label":"Checking\tthe header layout","doneAt":null}
    ]}"#;
}

pub mod gemini {
    pub const SESSION: &str = "g-7d21c0";

    pub const PROMPT: &str = r#"{"session_id":"g-7d21c0","transcript_path":"@ROOT@/home/.gemini/tmp/ledger/chats/session-7d21.json","cwd":"/home/robin/projects/ledger","hook_event_name":"BeforeAgent","timestamp":"2026-10-23T09:00:00Z","prompt":"Reconcile the March ledger against the bank export"}"#;

    pub const TOOL_USED: &str = r#"{"session_id":"g-7d21c0","transcript_path":"@ROOT@/home/.gemini/tmp/ledger/chats/session-7d21.json","cwd":"/home/robin/projects/ledger","hook_event_name":"AfterTool","tool_name":"read_file","tool_input":{"absolute_path":"/home/robin/projects/ledger/march.csv"}}"#;

    pub const ANSWERED: &str = r#"{"session_id":"g-7d21c0","transcript_path":"@ROOT@/home/.gemini/tmp/ledger/chats/session-7d21.json","cwd":"/home/robin/projects/ledger","hook_event_name":"AfterAgent","prompt":"Reconcile the March ledger against the bank export","prompt_response":"Two entries differ:\n  the 4th (rounding) and the 19th (a refund)."}"#;

    pub const PERMISSION: &str = r#"{"session_id":"g-7d21c0","cwd":"/home/robin/projects/ledger","hook_event_name":"Notification","notification_type":"ToolPermission","message":"Gemini CLI wants to run: rm -rf build/","details":{"type":"exec"}}"#;
}

pub mod codex {
    pub const SESSION: &str = "019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b";

    pub const PROMPT: &str = r#"{"session_id":"019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b","transcript_path":"@ROOT@/home/.codex/sessions/2026/10/23/rollout-019a2b3c.jsonl","cwd":"/home/robin/projects/orchard","hook_event_name":"UserPromptSubmit","model":"gpt-5-codex","turn_id":"t1","prompt":"Add pruning dates to the orchard planner"}"#;

    pub const PERMISSION: &str = r#"{"session_id":"019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b","cwd":"/home/robin/projects/orchard","hook_event_name":"PermissionRequest","tool_name":"shell","tool_input":{"command":["cargo","test","--","--include-ignored"]}}"#;

    pub const TOOL_ONLY: &str = r#"{"session_id":"019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b","cwd":"/home/robin/projects/orchard","hook_event_name":"PermissionRequest","tool_name":"apply_patch"}"#;

    pub const STOP: &str = r#"{"session_id":"019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b","cwd":"/home/robin/projects/orchard","hook_event_name":"Stop","last_assistant_message":"Pruning dates are in. Apples > pears, as asked."}"#;
}

pub mod agy {
    pub const SESSION: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";

    pub const SESSION_START: &str = r#"{"conversationId":"a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d","workspacePaths":["/home/robin/projects/greenhouse"],"transcriptPath":"@ROOT@/home/.gemini/antigravity/brain/a1b2c3d4/transcript.jsonl","hookEventName":"SessionStart"}"#;

    pub const STEP: &str = r#"{"conversationId":"a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d","workspacePaths":["/home/robin/projects/greenhouse"],"transcriptPath":"@ROOT@/home/.gemini/antigravity/brain/a1b2c3d4/transcript.jsonl","hookEventName":"PreInvocation"}"#;

    pub const STOP: &str = r#"{"conversationId":"a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d","workspacePaths":["/home/robin/projects/greenhouse"],"transcriptPath":"@ROOT@/home/.gemini/antigravity/brain/a1b2c3d4/transcript.jsonl","hookEventName":"Stop"}"#;

    pub const TRANSCRIPT_PATH: &str = "home/.gemini/antigravity/brain/a1b2c3d4/transcript.jsonl";

    pub const TRANSCRIPT: &str = r#"{"type":"USER_INPUT","content":"<USER_REQUEST>\nWater the seedlings on a timer\n</USER_REQUEST>"}
{"type":"PLANNER_RESPONSE","content":"Adding a watering schedule."}
{"type": "USER_INPUT", "content": "<ADDITIONAL_METADATA>open file: plan.md</ADDITIONAL_METADATA><USER_REQUEST>  Log the humidity\nevery hour too  \n</USER_REQUEST>"}
"#;
}

pub mod usage {
    /// Claude Code's login, valid until 2100.
    pub const CLAUDE_LOGIN: &str = r#"{"claudeAiOauth":{"accessToken":"sk-test-not-a-real-token","refreshToken":"never-read","expiresAt":4102444800000,"subscriptionType":"max"}}"#;

    pub const CLAUDE_EXPIRED: &str = r#"{"claudeAiOauth":{"accessToken":"sk-test-old","expiresAt":1000,"subscriptionType":"pro"}}"#;

    pub const CLAUDE_REPLY: &str = r#"{"limits":[
        {"kind":"session","group":"session","percent":76.4,"resets_at":"2099-01-01T05:00:00.123456+00:00"},
        {"kind":"weekly_all","group":"weekly","percent":12,"resets_at":"2099-01-05T00:00:00Z"},
        {"kind":"weekly_opus","group":"weekly","percent":91,"scope":{"model":{"display_name":"Opus"}},"resets_at":"2099-01-05T01:00:00+01:00"},
        {"kind":"extra_usage","percent":null,"resets_at":null}
    ]}"#;

    pub const CODEX_REPLIES: &str = r#"{"id":0,"result":{"userAgent":"codex"}}
{"method":"account/updated","params":{}}
{"id":1,"result":{"rateLimits":{"limitId":"codex","planType":"plus","primary":{"usedPercent":42.5,"windowDurationMins":300,"resetsAt":4070908800},"secondary":{"usedPercent":100,"windowDurationMins":10080,"resetsAt":4071513600}},"rateLimitsByLimitId":{"codex":{"limitId":"codex"},"codex-spark":{"limitName":"Spark","primary":{"usedPercent":3,"windowDurationMins":1440,"resetsAt":null}}}}}
"#;

    pub const AGY_READOUT: &str = "Your quotas\nGemini Pro Models\tDaily Limit Remaining\t20%\t2099-01-02T00:00:00Z\nClaude Models\tWeekly Limit Remaining\t95.5%\t2099-01-08T00:00:00Z\n";
}
