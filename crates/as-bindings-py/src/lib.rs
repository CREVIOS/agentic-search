//! Python bindings. v0 surfaces a single `search` stub; M5 wires the planner.

use pyo3::prelude::*;

#[pyfunction]
#[pyo3(signature = (query, _k=None))]
fn search(query: &str, _k: Option<usize>) -> PyResult<String> {
    Ok(format!("agentic-search: stub for query={query:?}"))
}

#[pymodule]
fn agentic_search(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(search, m)?)?;
    Ok(())
}
