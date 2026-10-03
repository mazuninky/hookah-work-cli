//! `hw product` arguments.

use clap::{Args, Subcommand, ValueEnum};

use super::{ApiName, PageArgs};

/// Products and categories.
#[derive(Debug, Clone, Subcommand)]
pub enum ProductCommand {
    /// Products available for sale, sorted by name (paginated)
    List(ProductListArgs),
    /// Product categories, in interface order
    Categories,
}

/// `hw product list`.
#[derive(Debug, Clone, Args)]
pub struct ProductListArgs {
    /// Only products of this category (ids: `hw product categories`)
    #[arg(long, value_name = "ID")]
    pub category: Option<u64>,

    /// Only products of this type
    #[arg(long = "type", value_name = "TYPE")]
    pub kind: Option<ProductType>,

    /// Only products whose name contains TEXT
    #[arg(long, value_name = "TEXT")]
    pub search: Option<String>,

    /// Nested data to include (comma-separated or repeated)
    #[arg(long, value_name = "FIELD", value_delimiter = ',')]
    pub expand: Vec<ProductExpand>,

    #[command(flatten)]
    #[allow(missing_docs)]
    pub page: PageArgs,
}

/// Product type filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProductType {
    /// Goods sold as they are
    Product,
    /// Dishes made from ingredients
    Dish,
    /// Ingredients of dishes
    Ingredient,
    /// Semi-finished products
    Semi,
}

impl ApiName for ProductType {
    fn api_name(&self) -> &'static str {
        match self {
            ProductType::Product => "product",
            ProductType::Dish => "dish",
            ProductType::Ingredient => "ingredient",
            ProductType::Semi => "semi",
        }
    }
}

/// `--expand` values of `hw product list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProductExpand {
    /// Composition of dishes and semi-finished products
    Components,
}

impl ApiName for ProductExpand {
    fn api_name(&self) -> &'static str {
        match self {
            ProductExpand::Components => "components",
        }
    }
}
