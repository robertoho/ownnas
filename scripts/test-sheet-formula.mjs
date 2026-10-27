// Checks for the spreadsheet formula engine and number formats (web/sheet-formula.js). Needs Node 20+.
import assert from 'node:assert/strict';
import { CellError, Unsupported, dateSerial, evaluate, formatValue, parseFormula, shiftFormula } from '../web/sheet-formula.js';

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
console.log('Formula, number format, and reference-shifting tests passed.');
