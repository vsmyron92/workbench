import { describe, expect, it } from 'vitest'
import { numericColumn, positionToLineColumn, qualified, quoteIdent, sourceLabel, splitStatements, statementAt, toTsv } from './logic'

describe('splitting SQL', () => {
  it('splits at top-level semicolons only', () => {
    const sql = `select 'a;b', "x;y" from t; -- c;d
insert into t values (E'it\\'s; fine');
/* a; /* nested; */ b; */ select 1;
do $$ begin raise notice 'x;'; end $$;
create function f() returns int as $body$ select 1; $body$ language sql;
select 2`
    const s = splitStatements(sql).map((x) => x.text)
    expect(s).toHaveLength(6)
    expect(s[0]).toBe(`select 'a;b', "x;y" from t`)
    expect(s[1]).toBe(`-- c;d\ninsert into t values (E'it\\'s; fine')`)
    expect(s[2]).toBe('/* a; /* nested; */ b; */ select 1')
    expect(s[3]).toBe("do $$ begin raise notice 'x;'; end $$")
    expect(s[4]).toContain('$body$ select 1; $body$')
    expect(s[5]).toBe('select 2')
  })
  it('keeps doubled quotes and ignores a $ inside identifiers', () => {
    expect(splitStatements("select 'it''s;'; select a$b; select 3").map((x) => x.text)).toEqual(["select 'it''s;'", 'select a$b', 'select 3'])
    expect(splitStatements('  ;  ; ')).toEqual([])
  })
  it('finds the statement at the caret', () => {
    const sql = 'select 1;\n\nselect 2;\nselect 3'
    expect(statementAt(sql, 3)?.text).toBe('select 1')
    expect(statementAt(sql, 9)?.text).toBe('select 1') // just after the ;
    expect(statementAt(sql, 10)?.text).toBe('select 1') // the blank line below
    expect(statementAt(sql, 14)?.text).toBe('select 2')
    expect(statementAt(sql, sql.length)?.text).toBe('select 3')
    expect(statementAt('', 0)).toBeNull()
    const s = statementAt(sql, 14)!
    expect(sql.slice(s.start, s.end)).toBe('select 2')
  })
})

describe('identifiers and results', () => {
  it('quotes what needs it', () => {
    expect(quoteIdent('orders')).toBe('orders')
    expect(quoteIdent('Order Items')).toBe('"Order Items"')
    expect(quoteIdent('user')).toBe('"user"')
    expect(quoteIdent('we"ird')).toBe('"we""ird"')
    expect(qualified('public', 'Users')).toBe('public."Users"')
  })
  it('copies as TSV and spots numeric columns', () => {
    expect(toTsv(['a', 'b'], [['1', null], ['x\ty', 'l1\nl2']])).toBe('a\tb\n1\t\nx\\ty\tl1\\nl2')
    expect(numericColumn([['1'], ['-2.5'], [null], ['3e5']], 0)).toBe(true)
    expect(numericColumn([['1'], ['x']], 0)).toBe(false)
    expect(numericColumn([[null]], 0)).toBe(false)
  })
  it('maps an error position to line and column', () => {
    expect(positionToLineColumn('select\n  nope from t', 10)).toEqual({ line: 2, column: 3 })
    expect(positionToLineColumn('select nope', 8)).toEqual({ line: 1, column: 8 })
  })
  it('labels sources', () => {
    expect(sourceLabel({ host: 'db.example.com', port: 5433, database: 'shop', user: 'app', url: '' })).toBe('app@db.example.com:5433/shop')
    expect(sourceLabel({ host: '', port: null, database: '', user: '', url: 'db_url' })).toBe('from secret db_url')
    expect(sourceLabel({ host: '', port: null, database: 'x', user: '', url: '' })).toBe('localhost/x')
  })
})
