import { useEffect, useMemo, useState } from 'react'
import { Button } from '@ui-sdk/react/button'
import { Input } from '@ui-sdk/react/input'
import { usePaginatedQuery } from 'convex/react'
import { api } from '../../convex/_generated/api'

const pageSize = 60
const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' })

export function Notes() {
  const { results, status, loadMore } = usePaginatedQuery(api.notes.listPage, {}, {
    initialNumItems: pageSize,
  })
  const [search, setSearch] = useState('')
  const [selectedKey, setSelectedKey] = useState<string>()

  useEffect(() => {
    if (status === 'CanLoadMore') loadMore(pageSize)
  }, [loadMore, status])

  const notes = useMemo(
    () => results.filter(note => !note.deleted).sort((a, b) => b.updated_at - a.updated_at),
    [results],
  )
  const selected = notes.find(note => note.key === selectedKey)
  const term = search.trim().toLowerCase()
  const matches = term
    ? notes.filter(note => note.title.toLowerCase().includes(term) || note.body.toLowerCase().includes(term))
    : notes
  const loading = status !== 'Exhausted'

  return (
    <div className="note-workspace">
      <section className="note-sidebar" aria-label="Notes list">
        <div className="search-area">
          <span aria-hidden="true" className="search-icon" />
          <Input
            className="search"
            type="search"
            aria-label="Search notes"
            placeholder="Search notes"
            value={search}
            onValueChange={setSearch}
          />
        </div>
        <div className="note-results">
          {matches.length > 0 && (
            <ul className="note-list">
              {matches.map(note => (
                <li key={note.key}>
                  <Button
                    className="note-row"
                    aria-label={`Open ${note.title.trim() || 'Untitled note'}`}
                    aria-current={note.key === selected?.key ? 'true' : undefined}
                    onClick={() => setSelectedKey(note.key)}
                  >
                    <span className="note-title">{note.title.trim() || 'Untitled note'}</span>
                    {note.body.trim() && <span className="note-preview">{note.body.trim()}</span>}
                  </Button>
                </li>
              ))}
            </ul>
          )}
          {loading && <p className="list-status" role="status">Loading notes…</p>}
          {!loading && matches.length === 0 && (
            <p className="list-status" role="status">
              {term ? 'No notes match your search.' : 'Your synced notes will appear here.'}
            </p>
          )}
        </div>
      </section>
      <section className="note-reader" aria-label="Note reader">
        {selected ? (
          <article key={selected.key} className="note-document">
            <h2>{selected.title.trim() || 'Untitled note'}</h2>
            {Number.isFinite(new Date(selected.updated_at).valueOf()) && (
              <p className="note-date">Edited {dateFormat.format(selected.updated_at)}</p>
            )}
            <div className="note-body">{selected.body}</div>
          </article>
        ) : (
          <div className="reader-empty"><p>Select a note to read it.</p></div>
        )}
      </section>
    </div>
  )
}
