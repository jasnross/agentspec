use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum Spec {
    Agent(AgentSpec),
    Skill(SkillSpec),
    Rule(RuleSpec),
}

impl Spec {
    pub fn body(&self) -> &str {
        match self {
            Spec::Agent(agent_spec) => &agent_spec.body,
            Spec::Skill(skill_spec) => &skill_spec.body,
            Spec::Rule(rule_spec) => &rule_spec.body,
        }
    }

    pub fn path(&self) -> &PathBuf {
        match self {
            Spec::Agent(agent_spec) => &agent_spec.path,
            Spec::Skill(skill_spec) => &skill_spec.path,
            Spec::Rule(rule_spec) => &rule_spec.path,
        }
    }
}

#[derive(Clone)]
pub enum NormalizedSpec {
    Agent(NormalizedAgentSpec),
    Skill(NormalizedSkillSpec),
    Rule(NormalizedRuleSpec),
}

impl NormalizedSpec {
    pub fn id(&self) -> &String {
        match self {
            NormalizedSpec::Agent(agent_spec) => &agent_spec.frontmatter.id,
            NormalizedSpec::Skill(skill_spec) => &skill_spec.frontmatter.id,
            NormalizedSpec::Rule(rule_spec) => &rule_spec.frontmatter.id,
        }
    }

    pub fn body(&self) -> &str {
        match self {
            NormalizedSpec::Agent(agent_spec) => &agent_spec.body,
            NormalizedSpec::Skill(skill_spec) => &skill_spec.body,
            NormalizedSpec::Rule(rule_spec) => &rule_spec.body,
        }
    }

    pub fn path(&self) -> &PathBuf {
        match self {
            NormalizedSpec::Agent(agent_spec) => &agent_spec.path,
            NormalizedSpec::Skill(skill_spec) => &skill_spec.path,
            NormalizedSpec::Rule(rule_spec) => &rule_spec.path,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AgentSpec {
    /// Absolute path to the spec
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: AgentFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
}

#[derive(Clone)]
pub struct NormalizedAgentSpec {
    /// Absolute path to the spec
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: NormalizedAgentFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
}

#[derive(Debug)]
pub struct SkillSpec {
    /// Absolute path to the spec root
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: SkillFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
    /// Additional files bundled with the skill
    pub supporting_files: Vec<SupportingFile>,
}

#[derive(Clone)]
pub struct NormalizedSkillSpec {
    /// Absolute path to the spec root
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: NormalizedSkillFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
    /// Additional files bundled with the skill
    pub supporting_files: Vec<SupportingFile>,
}

#[derive(Debug)]
pub struct RuleSpec {
    /// Absolute path to the spec root
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: RuleFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
}

#[derive(Clone)]
pub struct NormalizedRuleSpec {
    /// Absolute path to the spec root
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: NormalizedRuleFrontmatter,
    /// Spec body (Markdown content after frontmatter)
    pub body: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AgentFrontmatter {
    pub id: String,
    pub description: String,
    pub execution: Option<ExecutionFrontmatter>,
    pub capabilities: Option<CapabilitiesFrontmatter>,
}

#[derive(Clone)]
pub struct NormalizedAgentFrontmatter {
    pub id: String,
    pub description: String,
    pub execution: Option<ExecutionFrontmatter>,
    pub capabilities: Option<CapabilitiesFrontmatter>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SkillFrontmatter {
    pub id: String,
    pub description: Option<String>,
    pub user_invocable: bool,
    pub agent_invocable: bool,
    pub execution: Option<ExecutionFrontmatter>,
    pub capabilities: Option<CapabilitiesFrontmatter>,
}

#[derive(Clone)]
pub struct NormalizedSkillFrontmatter {
    pub id: String,
    pub description: Option<String>,
    pub user_invocable: bool,
    pub agent_invocable: bool,
    pub execution: Option<ExecutionFrontmatter>,
    pub capabilities: Option<CapabilitiesFrontmatter>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RuleFrontmatter {
    pub id: String,
    pub description: Option<String>,
}

#[derive(Clone)]
pub struct NormalizedRuleFrontmatter {
    pub id: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CapabilitiesFrontmatter {
    pub tools: Option<Vec<ToolFrontmatter>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecutionFrontmatter {
    pub preset: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, strum::VariantArray)]
pub enum ToolFrontmatter {
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "write")]
    Write,
    #[serde(rename = "edit")]
    Edit,
    #[serde(rename = "grep")]
    Grep,
    #[serde(rename = "glob")]
    Glob,
    #[serde(rename = "bash")]
    Bash,
    #[serde(rename = "webfetch")]
    WebFetch,
    #[serde(rename = "websearch")]
    WebSearch,
    #[serde(rename = "question")]
    Question,
    #[serde(rename = "tasks")]
    Tasks,
}

#[derive(Clone, Debug)]
pub struct SupportingFile {
    /// Path relative to the skill directory
    pub relative_path: PathBuf,
    /// Raw file content
    pub content: Vec<u8>,
    /// Whether the file has executable permission
    pub executable: bool,
}
