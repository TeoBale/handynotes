import { useState } from 'react'
import { useAuthActions } from '@convex-dev/auth/react'
import { createRootRoute, createRoute, createRouter, Link, Navigate, Outlet } from '@tanstack/react-router'
import { Button } from '@ui-sdk/react/button'
import { useConvexAuth } from 'convex/react'
import { Notes } from './Notes'

const rootRoute = createRootRoute({
  component: AppLayout,
  notFoundComponent: () => (
    <main className="message">
      <h1>Page not found</h1>
      <Link className="action" to="/">Back to home</Link>
    </main>
  ),
})

const homeRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/',
  component: () => import.meta.env.VITE_CONVEX_URL ? <Home /> : <Welcome />,
  errorComponent: ({ reset }) => (
    <main className="message">
      <h1>Couldn’t load your notes</h1>
      <p>Check your connection and try again.</p>
      <Button className="action" onClick={reset}>Try again</Button>
    </main>
  ),
})

const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/login',
  component: () => import.meta.env.VITE_CONVEX_URL ? <Login /> : (
    <LoginScreen>
      <p>Sign-in is unavailable until this site is connected to Convex.</p>
    </LoginScreen>
  ),
})

export const router = createRouter({
  routeTree: rootRoute.addChildren([homeRoute, loginRoute]),
})

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}

function AppLayout() {
  return (
    <>
      <header className="site-header">
        <Link className="brand" to="/">
          <NoteMark />
          HandyNotes
        </Link>
      </header>
      <Outlet />
    </>
  )
}

function NoteMark() {
  return <span aria-hidden="true" className="note-mark" />
}

function Home() {
  const { isLoading, isAuthenticated } = useConvexAuth()
  if (isLoading) return <main className="message"><p role="status">Checking your session…</p></main>
  return isAuthenticated ? <Workspace /> : <Welcome />
}

function Welcome() {
  return (
    <main className="message welcome">
      <NoteMark />
      <h1>Your notes, at hand.</h1>
      <p>Find and read the notes you’ve synced from HandyNotes.</p>
      <Link to="/login" className="action">Sign in</Link>
    </main>
  )
}

function Login() {
  const { isLoading, isAuthenticated } = useConvexAuth()
  const { signIn } = useAuthActions()
  const [error, setError] = useState(false)

  if (isLoading) return <main className="message"><p role="status">Checking your session…</p></main>
  if (isAuthenticated) return <Navigate to="/" replace />

  return (
    <LoginScreen>
      <p>Use your GitHub account to access your synced notes.</p>
      <Button
        className="action"
        pendingAnnouncement="Connecting to GitHub"
        onClick={async () => {
          setError(false)
          try {
            await signIn('github', { redirectTo: '/' })
          } catch {
            setError(true)
          }
        }}
      >
        Continue with GitHub
      </Button>
      {error && <p className="error" role="alert">Couldn’t connect to GitHub. Try again.</p>}
    </LoginScreen>
  )
}

function LoginScreen({ children }: { children: React.ReactNode }) {
  return (
    <main className="message">
      <h1>Sign in to HandyNotes</h1>
      {children}
      <Link className="back-link" to="/">Back to home</Link>
    </main>
  )
}

function Workspace() {
  const { signOut } = useAuthActions()
  const [error, setError] = useState(false)
  return (
    <main className="workspace">
      <div className="workspace-heading">
        <h1>Your notes</h1>
        <Button
          className="action secondary"
          pendingAnnouncement="Signing out"
          onClick={async () => {
            setError(false)
            try {
              await signOut()
            } catch {
              setError(true)
            }
          }}
        >
          Sign out
        </Button>
      </div>
      {error && <p className="error" role="alert">Couldn’t sign out. Try again.</p>}
      <Notes />
    </main>
  )
}
