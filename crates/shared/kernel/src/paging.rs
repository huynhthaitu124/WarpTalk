//! Pagination primitives.

use serde::{Deserialize, Serialize};

const DEFAULT_PAGE_SIZE: u32 = 20;
const MAX_PAGE_SIZE: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageRequest {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_page_size")]
    #[serde(alias = "page_size")]
    pub page_size: u32,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    DEFAULT_PAGE_SIZE
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: DEFAULT_PAGE_SIZE,
        }
    }
}

impl PageRequest {
    /// Effective (1-based) page, at least 1.
    pub fn page(&self) -> u32 {
        self.page.max(1)
    }

    /// Effective page size clamped to 1..=100.
    pub fn page_size(&self) -> u32 {
        self.page_size.clamp(1, MAX_PAGE_SIZE)
    }

    pub fn limit(&self) -> i64 {
        i64::from(self.page_size())
    }

    pub fn offset(&self) -> i64 {
        i64::from(self.page() - 1) * self.limit()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub page_size: u32,
    pub total: i64,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, req: &PageRequest, total: i64) -> Self {
        Self {
            items,
            page: req.page(),
            page_size: req.page_size(),
            total,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let r: PageRequest = serde_json::from_str("{}").unwrap();
        assert_eq!((r.page(), r.page_size()), (1, 20));
        assert_eq!((r.offset(), r.limit()), (0, 20));
    }

    #[test]
    fn clamping_and_offset() {
        let r = PageRequest {
            page: 0,
            page_size: 0,
        };
        assert_eq!((r.page(), r.page_size(), r.offset()), (1, 1, 0));
        let r = PageRequest {
            page: 3,
            page_size: 1000,
        };
        assert_eq!((r.limit(), r.offset()), (100, 200));
        let r = PageRequest {
            page: u32::MAX,
            page_size: 100,
        };
        assert!(r.offset() > 0);
    }

    #[test]
    fn page_serializes() {
        let req = PageRequest {
            page: 2,
            page_size: 5,
        };
        let p = Page::new(vec![1, 2], &req, 7);
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["total"], 7);
        assert_eq!(v["page"], 2);
        assert_eq!(v["items"], serde_json::json!([1, 2]));
    }
}
