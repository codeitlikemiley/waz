use warpui::{Entity, ModelContext, SingletonEntity};

#[derive(Debug, Clone)]
pub struct AddonCreditsOption {
    pub credits: i32,
    pub price_usd_cents: i32,
}

impl AddonCreditsOption {
    pub fn rate(&self) -> f32 {
        self.price_usd_cents as f32 / self.credits as f32
    }
}

#[derive(Debug, Clone)]
pub struct PricingInfo {
    pub plans: Vec<PlanPricing>,
    pub addon_credits_options: Vec<AddonCreditsOption>,
}

#[derive(Debug, Clone)]
pub struct PlanPricing {
    pub plan: StripeSubscriptionPlan,
    pub monthly_plan_price_per_month_usd_cents: i32,
    pub yearly_plan_price_per_month_usd_cents: i32,
    pub request_limit: Option<i32>,
    pub max_team_size: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StripeSubscriptionPlan {
    Business,
    Lightspeed,
    Pro,
    Team,
    Turbo,
    Build,
    BuildBusiness,
    BuildMax,
    Other(String),
}

/// Global model of server-side price information.
///
/// In Waz, it is a local no-op stub: OSS channel. There is no cloud service to push price data.
/// Therefore, `pricing_info` usually remains `None` during the process life cycle, and all getters return `None`.
/// The model is temporarily reserved for a small number of request usage and billing-compatible call points, and can be deleted in its entirety after subsequent cloud cleanup is completed.
#[derive(Debug)]
pub struct PricingInfoModel {
    pricing_info: Option<PricingInfo>,
}

impl PricingInfoModel {
    pub fn new() -> Self {
        Self { pricing_info: None }
    }

    /// Updates the model with the latest pricing information from the server.
    pub fn update_pricing_info(&mut self, pricing_info: PricingInfo, ctx: &mut ModelContext<Self>) {
        self.pricing_info = Some(pricing_info);
        ctx.emit(PricingInfoModelEvent::PricingInfoUpdated);
    }

    /// Returns the pricing for a specific plan.
    pub fn plan_pricing(&self, plan: &StripeSubscriptionPlan) -> Option<&PlanPricing> {
        self.pricing_info
            .as_ref()?
            .plans
            .iter()
            .find(|p| &p.plan == plan)
    }

    pub fn addon_credits_options(&self) -> Option<&[AddonCreditsOption]> {
        self.pricing_info
            .as_ref()
            .map(|info| info.addon_credits_options.as_slice())
    }
}

impl Default for PricingInfoModel {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub enum PricingInfoModelEvent {
    PricingInfoUpdated,
}

impl Entity for PricingInfoModel {
    type Event = PricingInfoModelEvent;
}

impl SingletonEntity for PricingInfoModel {}
