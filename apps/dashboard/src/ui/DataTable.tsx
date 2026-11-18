import {
  createColumnHelper,
  type ReactTable,
  type RowData,
  tableFeatures,
  useTable,
} from "@tanstack/react-table";
import { ChevronLeft, ChevronRight } from "lucide-react";
import type { KeyboardEvent, MouseEvent, ReactNode } from "react";

import { cx } from "~/lib/cx";

import { Button } from "./Button";
import s from "./DataTable.module.css";

/** ColumnMeta carries layout hints the table reads from each column. */
export type ColumnMeta = {
  /** CSS width, such as 120 or "30%". Columns without one share the rest. */
  width?: number | string;
  align?: "start" | "end";
  /** Hidden below 860px wide, for secondary columns. */
  hideOnMobile?: boolean;
};

/**
 * features is the one feature set every dashboard table uses: core rows and
 * typed column meta. Sorting, filtering, and paging happen on the server.
 */
export const features = tableFeatures({ columnMeta: {} as ColumnMeta });

export type Features = typeof features;

/** columnsFor starts a typed column helper for rows of T. */
export const columnsFor = <T extends RowData>() => createColumnHelper<Features, T>();

export { useTable };

/**
 * DataTable renders a table built with useTable. Rows with onRowClick act
 * like links: click or Enter opens them, and ⌘/Ctrl-click opens a new tab.
 */
export function DataTable<T extends RowData>({
  table,
  onRowClick,
  empty,
  dim,
}: {
  table: ReactTable<Features, T>;
  onRowClick?: (row: T, newTab: boolean) => void;
  empty?: ReactNode;
  /** Fades rows while a new page loads over the old one. */
  dim?: boolean;
}) {
  const rows = table.getRowModel().rows;
  if (rows.length === 0 && empty) return <>{empty}</>;

  return (
    <div className={s.wrap}>
      <table className={cx(s.table, dim && s.dim)}>
        <colgroup>
          {table.getAllLeafColumns().map((column) => (
            <col
              key={column.id}
              className={cx(column.columnDef.meta?.hideOnMobile && s.mobileHidden)}
              style={{ width: column.columnDef.meta?.width ?? "auto" }}
            />
          ))}
        </colgroup>
        <thead>
          {table.getHeaderGroups().map((group) => (
            <tr key={group.id}>
              {group.headers.map((header) => {
                const meta = header.column.columnDef.meta;
                return (
                  <th
                    key={header.id}
                    scope="col"
                    className={cx(
                      s.th,
                      meta?.align === "end" && s.end,
                      meta?.hideOnMobile && s.mobileHidden,
                    )}
                  >
                    {header.isPlaceholder ? null : <table.FlexRender header={header} />}
                  </th>
                );
              })}
            </tr>
          ))}
        </thead>
        <tbody>
          {rows.map((row) => {
            const open = onRowClick
              ? {
                  tabIndex: 0,
                  onClick: (e: MouseEvent) => {
                    if ((e.target as HTMLElement).closest("a, button")) return;
                    onRowClick(row.original, e.metaKey || e.ctrlKey);
                  },
                  onKeyDown: (e: KeyboardEvent) => {
                    if (e.key === "Enter") onRowClick(row.original, e.metaKey || e.ctrlKey);
                  },
                }
              : {};
            return (
              <tr key={row.id} {...open} className={cx(s.tr, onRowClick && s.clickable)}>
                {row.getAllCells().map((cell) => {
                  const meta = cell.column.columnDef.meta;
                  return (
                    <td
                      key={cell.id}
                      className={cx(
                        s.td,
                        meta?.align === "end" && s.end,
                        meta?.hideOnMobile && s.mobileHidden,
                      )}
                    >
                      <table.FlexRender cell={cell} />
                    </td>
                  );
                })}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/**
 * Pager pages through a cursor-paginated list (design §14.1). The API sends
 * no totals, so it shows the page number and moves one page at a time.
 */
export function Pager({
  page,
  hasPrevious,
  hasNext,
  onPrevious,
  onNext,
}: {
  page: number;
  hasPrevious: boolean;
  hasNext: boolean;
  onPrevious: () => void;
  onNext: () => void;
}) {
  if (!hasPrevious && !hasNext) return null;
  return (
    <div className={s.pager}>
      <span className={s.range}>Page {page.toLocaleString()}</span>
      <Button
        variant="secondary"
        size="sm"
        aria-label="Previous page"
        disabled={!hasPrevious}
        onClick={onPrevious}
        icon={<ChevronLeft size={16} />}
      />
      <Button
        variant="secondary"
        size="sm"
        aria-label="Next page"
        disabled={!hasNext}
        onClick={onNext}
        icon={<ChevronRight size={16} />}
      />
    </div>
  );
}
