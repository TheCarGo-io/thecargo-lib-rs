use uuid::Uuid;

// Which company's rows a request may see: every company (a service call that named none), only the
// organization's own rows, or one company.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanyRef {
    Any,
    Organization,
    Company(Uuid),
}

impl CompanyRef {
    pub fn from_company_id(company_id: Option<Uuid>) -> Self {
        company_id.map_or(Self::Organization, Self::Company)
    }
}
