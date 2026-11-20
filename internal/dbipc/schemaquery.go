package dbipc

// SchemaQuery couples a schema metadata statement with its bound parameters.
//
// Metadata filters (database/schema/table/view names) arrive from the host UI
// and must never be interpolated into SQL text: drivers that can bind values
// fill the `*Q` variants on SchemaSQL so the executor passes them as query
// arguments. The legacy string functions remain for drivers whose queries are
// fully static or whose dialect cannot express the filters as parameters.
type SchemaQuery struct {
	SQL  string
	Args []any
}

func staticQuery(sqlText string) SchemaQuery {
	return SchemaQuery{SQL: sqlText}
}

// databasesQuery resolves the database-list statement, preferring the
// args-bound variant.
func (s SchemaSQL) databasesQuery(cfg Config) SchemaQuery {
	if s.DatabasesQ != nil {
		return s.DatabasesQ(cfg)
	}
	if s.Databases == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.Databases(cfg))
}

func (s SchemaSQL) schemasQuery(cfg Config, database string) SchemaQuery {
	if s.SchemasQ != nil {
		return s.SchemasQ(cfg, database)
	}
	if s.Schemas == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.Schemas(cfg, database))
}

func (s SchemaSQL) objectsQuery(cfg Config, database, schema string, kinds []string) SchemaQuery {
	if s.ObjectsQ != nil {
		return s.ObjectsQ(cfg, database, schema, kinds)
	}
	if s.Objects == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.Objects(cfg, database, schema, kinds))
}

func (s SchemaSQL) columnsQuery(cfg Config, database, schema, table string) SchemaQuery {
	if s.ColumnsQ != nil {
		return s.ColumnsQ(cfg, database, schema, table)
	}
	if s.Columns == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.Columns(cfg, database, schema, table))
}

func (s SchemaSQL) indexesQuery(cfg Config, database, schema, table string) SchemaQuery {
	if s.IndexesQ != nil {
		return s.IndexesQ(cfg, database, schema, table)
	}
	if s.Indexes == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.Indexes(cfg, database, schema, table))
}

func (s SchemaSQL) foreignKeysQuery(cfg Config, database, schema, table string) SchemaQuery {
	if s.ForeignKeysQ != nil {
		return s.ForeignKeysQ(cfg, database, schema, table)
	}
	if s.ForeignKeys == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.ForeignKeys(cfg, database, schema, table))
}

func (s SchemaSQL) viewsQuery(cfg Config, database, schema string) SchemaQuery {
	if s.ViewsQ != nil {
		return s.ViewsQ(cfg, database, schema)
	}
	if s.Views == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.Views(cfg, database, schema))
}

func (s SchemaSQL) functionsQuery(cfg Config, database, schema string) SchemaQuery {
	if s.FunctionsQ != nil {
		return s.FunctionsQ(cfg, database, schema)
	}
	if s.Functions == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.Functions(cfg, database, schema))
}

func (s SchemaSQL) proceduresQuery(cfg Config, database, schema string) SchemaQuery {
	if s.ProceduresQ != nil {
		return s.ProceduresQ(cfg, database, schema)
	}
	if s.Procedures == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.Procedures(cfg, database, schema))
}

func (s SchemaSQL) triggersQuery(cfg Config, database, schema, table string) SchemaQuery {
	if s.TriggersQ != nil {
		return s.TriggersQ(cfg, database, schema, table)
	}
	if s.Triggers == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.Triggers(cfg, database, schema, table))
}

func (s SchemaSQL) sequencesQuery(cfg Config, database, schema string) SchemaQuery {
	if s.SequencesQ != nil {
		return s.SequencesQ(cfg, database, schema)
	}
	if s.Sequences == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.Sequences(cfg, database, schema))
}

func (s SchemaSQL) viewDefinitionQuery(cfg Config, database, schema, view string) SchemaQuery {
	if s.ViewDefinitionQ != nil {
		return s.ViewDefinitionQ(cfg, database, schema, view)
	}
	if s.ViewDefinition == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.ViewDefinition(cfg, database, schema, view))
}

func (s SchemaSQL) dumpDDLQuery(cfg Config, database, schema, table string) SchemaQuery {
	if s.DumpDDLQ != nil {
		return s.DumpDDLQ(cfg, database, schema, table)
	}
	if s.DumpDDL == nil {
		return SchemaQuery{}
	}
	return staticQuery(s.DumpDDL(cfg, database, schema, table))
}

// hasDatabases reports whether any variant of the database-list builder is
// set, gating the schema/databases handler.
func (s SchemaSQL) hasDatabases() bool {
	return s.Databases != nil || s.DatabasesQ != nil
}

func (s SchemaSQL) hasSchemas() bool {
	return s.Schemas != nil || s.SchemasQ != nil
}

func (s SchemaSQL) hasObjects() bool {
	return s.Objects != nil || s.ObjectsQ != nil
}

func (s SchemaSQL) hasColumns() bool {
	return s.Columns != nil || s.ColumnsQ != nil
}

func (s SchemaSQL) hasIndexes() bool {
	return s.Indexes != nil || s.IndexesQ != nil
}

func (s SchemaSQL) hasForeignKeys() bool {
	return s.ForeignKeys != nil || s.ForeignKeysQ != nil
}

func (s SchemaSQL) hasViews() bool {
	return s.Views != nil || s.ViewsQ != nil
}

func (s SchemaSQL) hasFunctions() bool {
	return s.Functions != nil || s.FunctionsQ != nil
}

func (s SchemaSQL) hasProcedures() bool {
	return s.Procedures != nil || s.ProceduresQ != nil
}

func (s SchemaSQL) hasTriggers() bool {
	return s.Triggers != nil || s.TriggersQ != nil
}

func (s SchemaSQL) hasSequences() bool {
	return s.Sequences != nil || s.SequencesQ != nil
}

func (s SchemaSQL) hasViewDefinition() bool {
	return s.ViewDefinition != nil || s.ViewDefinitionQ != nil
}

func (s SchemaSQL) hasDumpDDL() bool {
	return s.DumpDDL != nil || s.DumpDDLQ != nil
}
