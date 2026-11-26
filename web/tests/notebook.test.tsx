import { cleanup, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { RouterProvider } from '@tanstack/react-router'
import { createAgent } from '@ui-sdk/react/agent'
import { afterEach, beforeAll, expect, test, vi } from 'vitest'
import { router } from '../src/App'

const auth = vi.hoisted(() => ({
  isAuthenticated: false,
  signIn: vi.fn(),
  signOut: vi.fn(),
}))
const loadMore = vi.hoisted(() => vi.fn())
const notes = [
  { key: '1', title: 'Shopping', body: 'Lemons', updated_at: 3, deleted: false },
  { key: '2', title: 'Recipes', body: 'Lemon tart', updated_at: 2, deleted: false },
  { key: '3', title: 'Discarded', body: 'Lemon sorbet', updated_at: 1, deleted: true },
]

vi.mock('convex/react', () => ({
  useConvexAuth: () => ({ isAuthenticated: auth.isAuthenticated, isLoading: false }),
  usePaginatedQuery: () => ({ results: notes, status: 'Exhausted', loadMore }),
}))
vi.mock('@convex-dev/auth/react', () => ({
  useAuthActions: () => auth,
}))

beforeAll(() => {
  window.scrollTo = vi.fn()
})

afterEach(() => {
  cleanup()
  auth.isAuthenticated = false
  vi.clearAllMocks()
})

test('visitors navigate to login and start GitHub sign-in', async () => {
  const user = userEvent.setup()
  router.history.replace('/')
  render(<RouterProvider router={router} />)
  await user.click(await screen.findByRole('link', { name: 'Sign in' }))
  await user.click(await screen.findByRole('button', { name: 'Continue with GitHub' }))
  expect(auth.signIn).toHaveBeenCalledWith('github', { redirectTo: '/' })
  expect(screen.queryByRole('searchbox')).toBeNull()
})

test('signed-in users search synced notes and open them alongside the results', async () => {
  auth.isAuthenticated = true
  const user = userEvent.setup()
  router.history.replace('/login')
  render(<RouterProvider router={router} />)
  const search = await screen.findByRole('searchbox', { name: 'Search notes' })
  expect(screen.queryByRole('button', { name: 'Open Discarded' })).toBeNull()
  expect(createAgent().snapshot().map(entry => entry.label)).toContain('Search notes')
  await user.type(search, 'TART')
  expect(screen.queryByRole('button', { name: 'Open Shopping' })).toBeNull()
  await user.click(screen.getByRole('button', { name: 'Open Recipes' }))
  expect(within(screen.getByRole('region', { name: 'Note reader' })).getByText('Lemon tart')).toBeTruthy()
  await user.clear(search)
  await user.type(search, 'Shopping')
  await user.click(screen.getByRole('button', { name: 'Open Shopping' }))
  expect(within(screen.getByRole('region', { name: 'Note reader' })).getByText('Lemons')).toBeTruthy()
  await user.type(search, 'x')
  expect(screen.getByText('No notes match your search.')).toBeTruthy()
  await user.click(screen.getByRole('button', { name: 'Sign out' }))
  expect(auth.signOut).toHaveBeenCalledOnce()
})
