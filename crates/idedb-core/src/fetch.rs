//! How much of a result a statement reads now, and the paging every driver
//! shares so they all stop at a limit the same way.

use crate::Row;

/// How much of a result [`Session::execute`](crate::Session::execute) or
/// [`Session::fetch_more`](crate::Session::fetch_more) reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fetch {
    /// Rows per `Rows` event.
    pub page_size: usize,
    /// Rows to read before pausing with the rest of the result left open;
    /// `None` reads to the end.
    pub limit: Option<usize>,
}

impl Fetch {
    /// The whole result.
    pub fn all(page_size: usize) -> Self {
        Self { page_size: page_size.max(1), limit: None }
    }

    /// The first `limit` rows; the rest stays open for `fetch_more`.
    pub fn first(limit: usize, page_size: usize) -> Self {
        Self { page_size: page_size.max(1), limit: Some(limit) }
    }
}

/// What to do with the row just given to [`Pager::push`].
#[derive(Debug, PartialEq)]
pub enum Paged {
    /// Taken; keep reading.
    Continue,
    /// Taken, and it completed this page: emit it, then keep reading.
    Page(Vec<Row>),
    /// Not taken: it is past the limit, so more rows follow. Stop reading,
    /// emit [`Pager::finish`], and keep this row for the next fetch.
    Overflow(Row),
}

/// Cuts a stream of rows into pages and stops at the fetch limit.
///
/// Knowing whether more rows follow takes reading one row past the limit;
/// the pager hands that row back ([`Paged::Overflow`]) for the driver to
/// hold until the next fetch, so the UI never shows "more" for a result that
/// ended exactly at the limit.
#[derive(Debug)]
pub struct Pager {
    page_size: usize,
    remaining: Option<usize>,
    page: Vec<Row>,
    taken: u64,
}

impl Pager {
    pub fn new(fetch: Fetch) -> Self {
        let page_size = fetch.page_size.max(1);
        Self { page_size, remaining: fetch.limit, page: Vec::with_capacity(page_size.min(4096)), taken: 0 }
    }

    /// How many rows to ask a batch source (such as a SQL `FETCH`) for next:
    /// what is left of the limit plus the row that tells whether more
    /// follow, at most a page.
    pub fn wanted(&self) -> usize {
        match self.remaining {
            Some(remaining) => (remaining + 1).min(self.page_size),
            None => self.page_size,
        }
    }

    pub fn push(&mut self, row: Row) -> Paged {
        match &mut self.remaining {
            Some(0) => return Paged::Overflow(row),
            Some(remaining) => *remaining -= 1,
            None => {}
        }
        self.taken += 1;
        self.page.push(row);
        if self.page.len() == self.page_size {
            Paged::Page(std::mem::replace(&mut self.page, Vec::with_capacity(self.page_size.min(4096))))
        } else {
            Paged::Continue
        }
    }

    /// The last, partial page (empty when the rows ended on a page boundary).
    pub fn finish(&mut self) -> Vec<Row> {
        std::mem::take(&mut self.page)
    }

    /// Rows taken so far, in emitted pages and the current one.
    pub fn taken(&self) -> u64 {
        self.taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    fn row(i: i64) -> Row {
        vec![Value::Int(i)]
    }

    /// Feeds `available` rows, emitting as a driver would; returns the pages
    /// and the row held back, if any.
    fn drain(fetch: Fetch, available: i64) -> (Vec<Vec<Row>>, Option<Row>) {
        let mut pager = Pager::new(fetch);
        let mut pages = Vec::new();
        for i in 0..available {
            match pager.push(row(i)) {
                Paged::Continue => {}
                Paged::Page(page) => pages.push(page),
                Paged::Overflow(held) => {
                    pages.push(pager.finish());
                    return (pages.into_iter().filter(|p| !p.is_empty()).collect(), Some(held));
                }
            }
        }
        pages.push(pager.finish());
        (pages.into_iter().filter(|p| !p.is_empty()).collect(), None)
    }

    #[test]
    fn pages_a_whole_result() {
        let (pages, held) = drain(Fetch::all(2), 5);
        assert_eq!(pages, vec![vec![row(0), row(1)], vec![row(2), row(3)], vec![row(4)]]);
        assert_eq!(held, None);
    }

    #[test]
    fn stops_at_the_limit_and_holds_the_next_row() {
        let (pages, held) = drain(Fetch::first(3, 2), 10);
        assert_eq!(pages, vec![vec![row(0), row(1)], vec![row(2)]]);
        assert_eq!(held, Some(row(3)));
    }

    #[test]
    fn a_result_ending_at_the_limit_has_nothing_more() {
        let (pages, held) = drain(Fetch::first(3, 10), 3);
        assert_eq!(pages, vec![vec![row(0), row(1), row(2)]]);
        assert_eq!(held, None);
    }

    #[test]
    fn asks_batch_sources_for_one_row_past_the_limit() {
        let mut pager = Pager::new(Fetch::first(3, 10));
        assert_eq!(pager.wanted(), 4);
        pager.push(row(0));
        assert_eq!(pager.wanted(), 3);
        assert_eq!(Pager::new(Fetch::first(500, 100)).wanted(), 100);
        assert_eq!(Pager::new(Fetch::all(100)).wanted(), 100);
        assert_eq!(Pager::new(Fetch::first(0, 100)).wanted(), 1);
    }

    #[test]
    fn counts_the_rows_it_took() {
        let mut pager = Pager::new(Fetch::first(2, 10));
        pager.push(row(0));
        pager.push(row(1));
        assert!(matches!(pager.push(row(2)), Paged::Overflow(_)));
        assert_eq!(pager.taken(), 2);
    }
}
