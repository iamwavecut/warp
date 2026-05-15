use super::{CTAButton, LaunchModalEvent, Slide};
use crate::terminal::view::OnboardingIntention;
use crate::ui_components::icons::Icon;
use crate::workspace::action::WorkspaceAction;
use crate::workspace::view::OnboardingTutorial;
use asset_macro::bundled_or_fetched_asset;
use markdown_parser::{FormattedTextFragment, FormattedTextLine};
use warpui::assets::asset_cache::AssetSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OzLaunchSlide {
    LocalAgents,
    AgentAutomations,
    AgentManagement,
    LocalFirst,
}

impl Slide for OzLaunchSlide {
    fn modal_title(&self) -> String {
        "Introducing Oz".to_string()
    }

    fn modal_subtext_paragraphs(&self) -> Vec<FormattedTextLine> {
        vec![FormattedTextLine::Line(vec![
            FormattedTextFragment::plain_text(
                "Local-first coding agents for your sessions, tools, and custom providers.",
            ),
        ])]
    }

    fn first() -> Self {
        OzLaunchSlide::LocalAgents
    }

    fn next(&self) -> Option<Self> {
        match self {
            OzLaunchSlide::LocalAgents => Some(OzLaunchSlide::AgentAutomations),
            OzLaunchSlide::AgentAutomations => Some(OzLaunchSlide::AgentManagement),
            OzLaunchSlide::AgentManagement => Some(OzLaunchSlide::LocalFirst),
            OzLaunchSlide::LocalFirst => None,
        }
    }

    fn prev(&self) -> Option<Self> {
        match self {
            OzLaunchSlide::LocalAgents => None,
            OzLaunchSlide::AgentAutomations => Some(OzLaunchSlide::LocalAgents),
            OzLaunchSlide::AgentManagement => Some(OzLaunchSlide::AgentAutomations),
            OzLaunchSlide::LocalFirst => Some(OzLaunchSlide::AgentManagement),
        }
    }

    fn display_text(&self) -> Option<&'static str> {
        Some(match self {
            OzLaunchSlide::LocalAgents => "Local agents",
            OzLaunchSlide::AgentAutomations => "Agent automations",
            OzLaunchSlide::AgentManagement => "Agent management",
            OzLaunchSlide::LocalFirst => "Local-first",
        })
    }

    fn short_label(&self) -> &'static str {
        match self {
            OzLaunchSlide::LocalAgents => "Local agents",
            OzLaunchSlide::AgentAutomations => "Agent automations",
            OzLaunchSlide::AgentManagement => "Agent management",
            OzLaunchSlide::LocalFirst => "Local-first",
        }
    }

    fn title(&self) -> &'static str {
        match self {
            OzLaunchSlide::LocalAgents => "Run agents in your local workspace",
            OzLaunchSlide::AgentAutomations => {
                "Orchestrate agents, turning Skills into automations"
            }
            OzLaunchSlide::AgentManagement => "Track local agents seamlessly",
            OzLaunchSlide::LocalFirst => "Bring your own model provider",
        }
    }

    fn title_icon(&self) -> Option<Icon> {
        None
    }

    fn content(&self) -> &'static str {
        match self {
            OzLaunchSlide::LocalAgents => {
                "Use agents from your own sessions with local tools, MCP, and custom OpenAI-compatible providers."
            }
            OzLaunchSlide::AgentAutomations => {
                "Oz agents can be defined using the standard Skills format and launched locally from the app."
            }
            OzLaunchSlide::AgentManagement => {
                "View local agent sessions in Warp, continue tasks locally, and steer agents with one click."
            }
            OzLaunchSlide::LocalFirst => {
                "Configure BYOK or an OpenAI-compatible endpoint and keep agent execution tied to this machine."
            }
        }
    }

    fn image(&self) -> AssetSource {
        // TODO: Replace with new images once provided.
        match self {
            OzLaunchSlide::LocalAgents => {
                bundled_or_fetched_asset!("png/oz_cloud_agents.png")
            }
            OzLaunchSlide::AgentAutomations => {
                bundled_or_fetched_asset!("png/oz_agent_automations.png")
            }
            OzLaunchSlide::AgentManagement => {
                bundled_or_fetched_asset!("png/oz_agent_management.png")
            }
            OzLaunchSlide::LocalFirst => {
                bundled_or_fetched_asset!("png/oz_launch_credits.png")
            }
        }
    }

    fn all() -> Vec<Self> {
        vec![
            OzLaunchSlide::LocalAgents,
            OzLaunchSlide::AgentAutomations,
            OzLaunchSlide::AgentManagement,
            OzLaunchSlide::LocalFirst,
        ]
    }

    fn cta_button(&self) -> CTAButton<Self> {
        match self {
            OzLaunchSlide::LocalAgents
            | OzLaunchSlide::AgentAutomations
            | OzLaunchSlide::AgentManagement => {
                let next = self.next().expect("Non-final slides should have a next");
                CTAButton::next_slide(next, format!("Next: {}", next.short_label()))
            }
            OzLaunchSlide::LocalFirst => CTAButton::custom("Try it out", |ctx| {
                ctx.emit(LaunchModalEvent::Close);
                ctx.dispatch_typed_action(&WorkspaceAction::StartAgentOnboardingTutorial(
                    OnboardingTutorial::NoProject {
                        intention: OnboardingIntention::AgentDrivenDevelopment,
                    },
                ));
                ctx.dispatch_typed_action(&WorkspaceAction::AddAgentTab);
            }),
        }
    }

    fn secondary_cta_button(&self) -> Option<CTAButton<Self>> {
        match self {
            OzLaunchSlide::LocalFirst => Some(CTAButton::close("Skip for now")),
            OzLaunchSlide::LocalAgents
            | OzLaunchSlide::AgentAutomations
            | OzLaunchSlide::AgentManagement => None,
        }
    }

    fn on_close(&self, ctx: &mut warpui::ViewContext<super::LaunchModal<Self>>) {
        ctx.dispatch_typed_action(&WorkspaceAction::StartAgentOnboardingTutorial(
            OnboardingTutorial::NoProject {
                intention: OnboardingIntention::AgentDrivenDevelopment,
            },
        ));
    }
}

pub fn init(app: &mut warpui::AppContext) {
    super::init::<OzLaunchSlide>(app);
}
