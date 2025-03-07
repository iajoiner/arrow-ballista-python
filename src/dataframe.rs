// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

use crate::utils::wait_for_future;
use crate::{errors::DataFusionError, expression::PyExpr};
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::pyarrow::{PyArrowConvert, PyArrowException, PyArrowType};
use datafusion::arrow::util::pretty;
use datafusion::dataframe::DataFrame;
use datafusion::logical_expr::JoinType;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::PyTuple;
use std::sync::Arc;

/// A PyDataFrame is a representation of a logical plan and an API to compose statements.
/// Use it to build a plan and `.collect()` to execute the plan and collect the result.
/// The actual execution of a plan runs natively on Rust and Arrow on a multi-threaded environment.
#[pyclass(name = "DataFrame", module = "ballista", subclass)]
#[derive(Clone)]
pub(crate) struct PyDataFrame {
    df: Arc<DataFrame>,
}

impl PyDataFrame {
    /// creates a new PyDataFrame
    pub fn new(df: DataFrame) -> Self {
        Self { df: Arc::new(df) }
    }
}

#[pymethods]
impl PyDataFrame {
    fn __getitem__(&self, key: PyObject) -> PyResult<Self> {
        Python::with_gil(|py| {
            if let Ok(key) = key.extract::<&str>(py) {
                self.select_columns(vec![key])
            } else if let Ok(tuple) = key.extract::<&PyTuple>(py) {
                let keys = tuple
                    .iter()
                    .map(|item| item.extract::<&str>())
                    .collect::<PyResult<Vec<&str>>>()?;
                self.select_columns(keys)
            } else if let Ok(keys) = key.extract::<Vec<&str>>(py) {
                self.select_columns(keys)
            } else {
                let message = "DataFrame can only be indexed by string index or indices";
                Err(PyTypeError::new_err(message))
            }
        })
    }

    /// Returns the schema from the logical plan
    fn schema(&self) -> PyArrowType<Schema> {
        PyArrowType(self.df.schema().into())
    }

    #[args(args = "*")]
    fn select_columns(&self, args: Vec<&str>) -> PyResult<Self> {
        let df = self.df.as_ref().clone().select_columns(&args)?;
        Ok(Self::new(df))
    }

    #[args(args = "*")]
    fn select(&self, args: Vec<PyExpr>) -> PyResult<Self> {
        let expr = args.into_iter().map(|e| e.into()).collect();
        let df = self.df.as_ref().clone().select(expr)?;
        Ok(Self::new(df))
    }

    fn filter(&self, predicate: PyExpr) -> PyResult<Self> {
        let df = self.df.as_ref().clone().filter(predicate.into())?;
        Ok(Self::new(df))
    }

    fn with_column(&self, name: &str, expr: PyExpr) -> PyResult<Self> {
        let df = self.df.as_ref().clone().with_column(name, expr.into())?;
        Ok(Self::new(df))
    }

    fn aggregate(&self, group_by: Vec<PyExpr>, aggs: Vec<PyExpr>) -> PyResult<Self> {
        let group_by = group_by.into_iter().map(|e| e.into()).collect();
        let aggs = aggs.into_iter().map(|e| e.into()).collect();
        let df = self.df.as_ref().clone().aggregate(group_by, aggs)?;
        Ok(Self::new(df))
    }

    #[args(exprs = "*")]
    fn sort(&self, exprs: Vec<PyExpr>) -> PyResult<Self> {
        let exprs = exprs.into_iter().map(|e| e.into()).collect();
        let df = self.df.as_ref().clone().sort(exprs)?;
        Ok(Self::new(df))
    }

    fn limit(&self, count: usize) -> PyResult<Self> {
        let df = self.df.as_ref().clone().limit(0, Some(count))?;
        Ok(Self::new(df))
    }

    /// Executes the plan, returning a list of `RecordBatch`es.
    /// Unless some order is specified in the plan, there is no
    /// guarantee of the order of the result.
    fn collect(&self, py: Python) -> PyResult<Vec<PyObject>> {
        let batches = wait_for_future(py, self.df.as_ref().clone().collect())?;
        // cannot use PyResult<Vec<RecordBatch>> return type due to
        // https://github.com/PyO3/pyo3/issues/1813
        batches.into_iter().map(|rb| rb.to_pyarrow(py)).collect()
    }

    /// Print the result, 20 lines by default
    #[args(num = "20")]
    fn show(&self, py: Python, num: usize) -> PyResult<()> {
        let df = self.df.as_ref().clone().limit(0, Some(num))?;
        let batches = wait_for_future(py, df.collect())?;
        pretty::print_batches(&batches)
            .map_err(|err| PyArrowException::new_err(err.to_string()))
    }

    fn join(
        &self,
        right: PyDataFrame,
        join_keys: (Vec<&str>, Vec<&str>),
        how: &str,
    ) -> PyResult<Self> {
        let join_type = match how {
            "inner" => JoinType::Inner,
            "left" => JoinType::Left,
            "right" => JoinType::Right,
            "full" => JoinType::Full,
            "semi" => JoinType::LeftSemi,
            "anti" => JoinType::LeftAnti,
            "right_semi" => JoinType::RightSemi,
            how => {
                return Err(DataFusionError::Common(format!(
                    "The join type {} does not exist or is not implemented",
                    how
                ))
                .into())
            }
        };

        let df = self.df.as_ref().clone().join(
            right.df.as_ref().clone(),
            join_type,
            &join_keys.0,
            &join_keys.1,
            None,
        )?;
        Ok(Self::new(df))
    }

    /// Print the explain output to stdout
    #[args(verbose = false, analyze = false)]
    fn explain(&self, py: Python, verbose: bool, analyze: bool) -> PyResult<()> {
        let df = self.df.as_ref().clone().explain(verbose, analyze)?;
        let batches = wait_for_future(py, df.collect())?;
        pretty::print_batches(&batches)
            .map_err(|err| PyArrowException::new_err(err.to_string()))
    }

    /// Get the explain output as a string
    #[args(verbose = false, analyze = false)]
    fn explain_string(
        &self,
        py: Python,
        verbose: bool,
        analyze: bool,
    ) -> PyResult<String> {
        let df = self.df.as_ref().clone().explain(verbose, analyze)?;
        let batches = wait_for_future(py, df.collect())?;
        let display = pretty::pretty_format_batches(&batches)
            .map_err(|err| PyArrowException::new_err(err.to_string()))?;
        Ok(format!("{}", display))
    }
}
