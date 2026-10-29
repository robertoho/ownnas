// Checks for the spreadsheet formula engine and number formats (web/sheet-formula.js). Needs Node 20+.
import assert from 'node:assert/strict';
import { CellError, Unsupported, dateSerial, evaluate, formatValue, fromFileFormula, parseFormula, shiftFormula, toFileFormula } from '../web/sheet-formula.js';

const format = (value, code) => formatValue(value, code, { decimal: '.', group: ',' }).text;
for (const [value, code, expected] of [
  [1250.5, '#,##0.00 "€"', '1,250.50 €'], [1234567.891, '#,##0.00', '1,234,567.89'], [0.6879, '0.0%', '68.8%'],
  [46023, 'yyyy\\-mm\\-dd', '2026-01-01'], [46023.75, 'yyyy-mm-dd hh:mm', '2026-01-01 18:00'], [-5, '#,##0;(#,##0)', '(5)'],
  [0, '0.00;-0.00;"zero"', 'zero'], [5551234, '000-0000', '555-1234'], [12345.678, '0.00E+00', '1.23E+04'], [5, '0.##', '5'],
  [0.5, 'h:mm AM/PM', '12:00 PM'], [1.5, '[h]:mm', '36:00'], [3.14159, 'General', '3.14159'], [-1234.5, '$#,##0.00', '-$1,234.50'],
  [1234.5, '[$€-407] #,##0.00', '€ 1,234.50'], [7, '00', '07'], [0, '#,##0', '0'], [0.25, '0%', '25%'], [123456789012, 'General', '1.23457E+11'],
  ['text', '@', 'text'], [true, 'General', 'TRUE'], [new CellError('#DIV/0!'), 'General', '#DIV/0!'],
]) assert.equal(format(value, code), expected, `${value} as ${code}`);
assert.equal(formatValue(-3, '0;[Red]-0').color, '#ff0000');

const cells = { A1: 10, A2: 20, A3: 'x', B1: 2, B2: true, C1: 'Apple', C2: 'Pear', D1: 1, D2: 2 };
const context = { date1904: false, row: 1, col: 5, size: () => ({ rows: 3, cols: 4 }), cell: (sheet, row, col) => cells[String.fromCharCode(64 + col) + row] ?? null };
const run = source => {
  const value = evaluate(parseFormula(source), context);
  return value instanceof CellError ? value.code : value;
};
for (const [source, expected] of [
  ['SUM(A1:A3)', 30], ['A1+B1*3', 16], ['-2^2', 4], ['2^3^2', 64], ['(A1+A2)/0', '#DIV/0!'], ['IF(A1>5,"big","small")', 'big'],
  ['A1&"-"&C1', '10-Apple'], ['AVERAGE(A1:A2)', 15], ['COUNT(A1:A3)', 2], ['COUNTA(A1:C3)', 7], ['VLOOKUP("pear",C1:D2,2,FALSE)', 2],
  ['SUMIF(A1:A2,">15")', 20], ['ROUND(2.345,2)', 2.35], ['IFERROR(1/0,"none")', 'none'], ['UPPER(C1)', 'APPLE'], ['50%*A1', 5],
  ['TEXT(0.5,"0%")', '50%'], ['YEAR(DATE(2026,1,1))', 2026], ['DATE(2026,1,1)', 46023], ['INDEX(C1:D2,2,1)', 'Pear'], ['MATCH(20,A1:A3,0)', 2],
  ['A1=10', true], ['"a"<"B"', true], ['MAX(A1:B2)', 20], ['TRUE()', true], ['C1:C2', '#VALUE!'], ['_xlfn.XLOOKUP("Pear",C1:C2,D1:D2)', 2],
]) assert.equal(run(source), expected, source);
for (const source of ['FOO(1)', 'MyName+1']) assert.throws(() => run(source), Unsupported, `${source} is left to the spreadsheet app`);

for (const [source, rows, cols, expected] of [
  ['C2/C$7', 1, 0, 'C3/C$7'], ['SUM($A1:B2)+Sheet2!A1', 2, 1, 'SUM($A3:C4)+Sheet2!B3'], ['"A1"&A1', 1, 1, '"A1"&B2'], ['A1', -1, 0, '#REF!'],
  ["'My sheet'!A1*2", 1, 0, "'My sheet'!A2*2"],
]) assert.equal(shiftFormula(source, rows, cols), expected, source);
assert.equal(dateSerial(1900, 1, 1), 1);
assert.equal(dateSerial(1900, 3, 1), 61);

// Newer functions: dates, statistics, arrays.
{
  const grid = {
    A1: 'b', A2: 'a', A3: 'b', A4: 'c', B1: 3, B2: 1, B3: 2, B4: 5,
    C1: dateSerial(2026, 1, 31), C2: dateSerial(2026, 3, 2), C3: dateSerial(2024, 2, 29), D1: dateSerial(2026, 1, 1),
  };
  const ctx = {
    date1904: false, row: 9, col: 9, size: () => ({ rows: 4, cols: 4 }),
    cell: (sheet, row, col) => grid[String.fromCharCode(64 + col) + row] ?? null,
    now: () => new Date(2026, 9, 29, 18, 0, 0),
  };
  const value = (source, options) => {
    const result = evaluate(parseFormula(source), ctx, options);
    return result instanceof CellError ? result.code : result;
  };
  for (const [source, expected] of [
    ['TODAY()', dateSerial(2026, 10, 29)], ['NOW()', dateSerial(2026, 10, 29) + 0.75],
    ['EDATE(C1,1)', dateSerial(2026, 2, 28)], ['EOMONTH(C1,1)', dateSerial(2026, 2, 28)], ['EDATE(C3,12)', dateSerial(2025, 2, 28)],
    ['EOMONTH(D1,-1)', dateSerial(2025, 12, 31)],
    ['NETWORKDAYS(D1,C1)', 22], ['NETWORKDAYS(C1,D1)', -22], ['NETWORKDAYS(D1,C1,D1)', 21],
    ['DATEDIF(C3,C2,"Y")', 2], ['DATEDIF(C3,C2,"M")', 24], ['DATEDIF(C3,C2,"YM")', 0], ['DATEDIF(C3,C2,"MD")', 1], ['DATEDIF(C2,C3,"D")', '#NUM!'],
    ['DATEDIF(D1,C1,"D")', 30],
    ['SUMPRODUCT(B1:B4,B1:B4)', 39], ['SUMPRODUCT((A1:A4="b")*B1:B4)', 5], ['SUMPRODUCT(--(B1:B4>1))', 3],
    ['ROUND(STDEV(B1:B4),6)', 1.707825], ['ROUND(STDEV.P(B1:B4),6)', 1.479020], ['STDEV(B1)', '#DIV/0!'],
    ['RANK(3,B1:B4)', 2], ['RANK.EQ(1,B1:B4,1)', 1], ['RANK(9,B1:B4)', '#N/A'],
    ['COUNTA(UNIQUE(A1:A4))', 3], ['SUM(FILTER(B1:B4,A1:A4="b"))', 5], ['INDEX(SORT(B1:B4,1,-1),1)', 5],
    ['FILTER(B1:B4,B1:B4>9)', '#CALC!'], ['FILTER(B1:B4,B1:B4>9,"none")', 'none'], ['UNIQUE(A1:A4)', '#VALUE!'],
  ]) assert.equal(value(source), expected, source);
  assert.deepEqual(value('UNIQUE(A1:A4)', { array: true }), { rows: [['b'], ['a'], ['c']] }, 'arrays come back whole to spill');
  assert.deepEqual(value('SORT(A1:B4,2,-1)', { array: true }).rows.map(row => row[0]), ['c', 'b', 'b', 'a']);
  assert.deepEqual(value('B1:B2*10', { array: true }), { rows: [[30], [10]] }, 'operators work cell by cell');
  const random = value('RANDBETWEEN(1,6)');
  assert.ok(random >= 1 && random <= 6 && Number.isInteger(random));
  assert.equal(toFileFormula('XLOOKUP(A1,B:B,C:C)+SUM(SORT(A1:A3))+_xlfn.CONCAT(A1)'), '_xlfn.XLOOKUP(A1,B:B,C:C)+SUM(_xlfn._xlws.SORT(A1:A3))+_xlfn.CONCAT(A1)', 'newer functions are saved with their prefix');
  assert.equal(fromFileFormula('_xlfn._xlws.FILTER(A1:A3,B1:B3)+_xlfn.STDEV.S(A1)'), 'FILTER(A1:A3,B1:B3)+STDEV.S(A1)');
  assert.equal(value('_xlfn._xlws.SORT(B1:B2)', { array: true }).rows[0][0], 1, 'and read with it');
}
console.log('Formula, number format, and reference-shifting tests passed.');
