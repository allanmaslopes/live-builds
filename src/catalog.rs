use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::num::ParseIntError;

pub struct Item {
    pub id: u32,
    pub name: String,
    pub total_cost: u32,
    pub base_cost: u32,
    pub from: Vec<u32>,
    pub purchasable: bool,
}

pub struct ItemCatalog {
    pub patch: String,
    items: HashMap<u32, Item>,
}

#[derive(Debug)]
pub enum CatalogError {
    Json(serde_json::Error),
    InvalidItemId {
        value: String,
        source: ParseIntError,
    },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "invalid Data Dragon JSON: {error}"),
            Self::InvalidItemId { value, .. } => write!(formatter, "invalid item id `{value}`"),
        }
    }
}

impl Error for CatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::InvalidItemId { source, .. } => Some(source),
        }
    }
}

impl From<serde_json::Error> for CatalogError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl ItemCatalog {
    pub fn from_ddragon_json(patch: &str, json: &str) -> Result<ItemCatalog, CatalogError> {
        let document: DDragonCatalog = serde_json::from_str(json)?;
        let mut items = HashMap::with_capacity(document.data.len());

        for (raw_id, raw_item) in document.data {
            let id = parse_item_id(&raw_id)?;
            let from = raw_item
                .from
                .iter()
                .map(|component| parse_item_id(component))
                .collect::<Result<Vec<_>, _>>()?;

            items.insert(
                id,
                Item {
                    id,
                    name: raw_item.name,
                    total_cost: raw_item.gold.total,
                    base_cost: raw_item.gold.base,
                    from,
                    purchasable: raw_item.gold.purchasable,
                },
            );
        }

        Ok(ItemCatalog {
            patch: patch.to_owned(),
            items,
        })
    }

    /// `None` means the item is unknown; consumers must never treat it as purchasable.
    pub fn get(&self, id: u32) -> Option<&Item> {
        self.items.get(&id)
    }

    pub fn name(&self, id: u32) -> String {
        self.get(id)
            .map_or_else(|| format!("Item {id}"), |item| item.name.clone())
    }

    /// `owned` contém `target` ou uma evolução gratuita dele (ex.: 3172 conta como 3006).
    /// Componentes consumidos numa receita não contam: viraram outro item.
    pub fn owns(&self, target: u32, owned: &[u32]) -> bool {
        owned.iter().any(|&id| self.is_or_evolves_from(id, target))
    }

    /// `id == target`, ou `id` vem de `target` só por evoluções gratuitas
    /// (receita de um único componente e `base_cost == 0`), em qualquer profundidade.
    pub fn is_or_evolves_from(&self, id: u32, target: u32) -> bool {
        let mut current = id;
        let mut visited = HashSet::new();
        while visited.insert(current) {
            if current == target {
                return true;
            }
            match self.get(current) {
                Some(item) if item.from.len() == 1 && item.base_cost == 0 => current = item.from[0],
                _ => return false,
            }
        }
        false
    }

    pub fn remaining_cost(&self, target: u32, owned: &[u32]) -> u32 {
        let Some(target_item) = self.get(target) else {
            return 0;
        };
        if owned.contains(&target) {
            return 0;
        }

        let mut available = HashMap::new();
        for &id in owned {
            *available.entry(id).or_insert(0) += 1;
        }

        let mut path = HashSet::from([target]);
        let discount = target_item.from.iter().fold(0_u32, |total, &component| {
            total.saturating_add(self.component_discount(component, &mut available, &mut path))
        });

        target_item.total_cost.saturating_sub(discount)
    }

    fn component_discount(
        &self,
        id: u32,
        available: &mut HashMap<u32, usize>,
        path: &mut HashSet<u32>,
    ) -> u32 {
        if let Some(count) = available.get_mut(&id)
            && *count > 0
        {
            *count -= 1;
            return self.get(id).map_or(0, |item| item.total_cost);
        }

        let Some(item) = self.get(id) else {
            return 0;
        };
        if !path.insert(id) {
            return 0;
        }

        let discount = item.from.iter().fold(0_u32, |total, &component| {
            total.saturating_add(self.component_discount(component, available, path))
        });
        path.remove(&id);
        discount
    }
}

#[derive(Deserialize)]
struct DDragonCatalog {
    data: HashMap<String, DDragonItem>,
}

#[derive(Deserialize)]
struct DDragonItem {
    name: String,
    gold: DDragonGold,
    #[serde(default)]
    from: Vec<String>,
}

#[derive(Deserialize)]
struct DDragonGold {
    total: u32,
    base: u32,
    purchasable: bool,
}

fn parse_item_id(value: &str) -> Result<u32, CatalogError> {
    value.parse().map_err(|source| CatalogError::InvalidItemId {
        value: value.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::ItemCatalog;

    fn catalog() -> ItemCatalog {
        ItemCatalog::from_ddragon_json(
            "16.22.1",
            include_str!("../tests/fixtures/item_sample.json"),
        )
        .expect("fixture should contain a valid Data Dragon item catalog")
    }

    #[test]
    fn parses_names_and_costs() {
        let catalog = catalog();
        let item = catalog.get(3071).expect("item 3071 should exist");

        assert_eq!(catalog.patch, "16.22.1");
        assert_eq!(item.name, "Cutelo Negro");
        assert_eq!(item.total_cost, 3000);
        assert_eq!(item.base_cost, 225);
        assert!(item.purchasable);
        assert_eq!(item.from, vec![3044, 3067, 1037]);
    }

    #[test]
    fn unknown_item_is_named_by_id() {
        assert_eq!(catalog().name(999999), "Item 999999");
    }

    #[test]
    fn remaining_cost_subtracts_owned_components() {
        assert_eq!(catalog().remaining_cost(3071, &[3044]), 1900);
    }

    #[test]
    fn remaining_cost_uses_each_owned_item_once() {
        assert_eq!(catalog().remaining_cost(3077, &[1036]), 850);
    }

    #[test]
    fn remaining_cost_nested_components() {
        assert_eq!(catalog().remaining_cost(3071, &[1028]), 2600);
    }

    #[test]
    fn remaining_cost_of_owned_item_is_zero() {
        assert_eq!(catalog().remaining_cost(3071, &[3071]), 0);
    }

    #[test]
    fn owns_through_upgrade() {
        let catalog = ItemCatalog::from_ddragon_json(
            "16.22.1",
            include_str!("../tests/fixtures/item_thresh.json"),
        )
        .expect("fixture should contain a valid Data Dragon item catalog");

        assert!(catalog.owns(3172, &[3172]));
        assert!(catalog.owns(3006, &[3172]));
    }

    #[test]
    fn owns_through_evolution_chain() {
        let catalog = ItemCatalog::from_ddragon_json(
            "16.22.1",
            include_str!("../tests/fixtures/item_thresh.json"),
        )
        .expect("fixture should contain a valid Data Dragon item catalog");

        assert!(catalog.owns(3010, &[3176]));
    }

    #[test]
    fn component_inside_owned_item_is_not_owned() {
        let catalog = ItemCatalog::from_ddragon_json(
            "16.22.1",
            include_str!("../tests/fixtures/item_thresh.json"),
        )
        .expect("fixture should contain a valid Data Dragon item catalog");

        assert!(!catalog.owns(1001, &[3006]));
        assert!(!catalog.owns(1018, &[3031]));
    }

    #[test]
    fn upgrade_is_not_owned_by_base() {
        let catalog = ItemCatalog::from_ddragon_json(
            "16.22.1",
            include_str!("../tests/fixtures/item_thresh.json"),
        )
        .expect("fixture should contain a valid Data Dragon item catalog");

        assert!(!catalog.owns(3172, &[3006]));
        assert!(!catalog.owns(3006, &[1001]));
        assert!(!catalog.owns(3047, &[3172]));
    }
}
