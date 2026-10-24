/// Default role files as `(file name, content)`, written on first use.
pub const DEFAULT_ROLES: [(&str, &str); 4] = [
    (
        "explorador.md",
        "+++\nname = \"explorador\"\ndescription = \"Reads code and summarizes. Never edits.\"\nmodel = \"@rapido\"\naccess = \"read-only\"\n+++\n\nRead the code relevant to the brief and report what you found. Never edit files.\n",
    ),
    (
        "planejador.md",
        "+++\nname = \"planejador\"\ndescription = \"Writes the spec and the plan. Does not implement.\"\nmodel = \"@forte\"\naccess = \"write\"\n+++\n\nWrite the spec and the plan the brief asks for. Do not implement.\n",
    ),
    (
        "executor.md",
        "+++\nname = \"executor\"\ndescription = \"Implements the brief and proves it works.\"\nmodel = \"@medio\"\naccess = \"write\"\n+++\n\nImplement exactly what the brief and the spec ask. Prove it works as the brief says. Do not change the contract; ask the orchestrator instead.\n",
    ),
    (
        "revisor.md",
        "+++\nname = \"revisor\"\ndescription = \"Reviews the diff and the tests against the spec.\"\nmodel = \"@forte\"\naccess = \"write\"\n+++\n\nCheck the diff and the tests against the spec. Report concrete findings with file and line. Do not fix them.\n",
    ),
];

/// Default model map written next to the default roles.
pub const DEFAULT_MODELS_TOML: &str = r#"[alias.forte]
claude-code = ["claude-opus-5-5:high", "claude-fable-5-1:high"]
codex = ["gpt-6-astra:high"]

[alias.medio]
claude-code = ["claude-sonnet-5-5:medium"]
codex = ["gpt-6.1-sol:medium"]

[alias.rapido]
claude-code = ["claude-haiku-5-5:low"]
codex = ["gpt-6-luna:low"]
"#;
