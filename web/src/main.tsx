import { ConvexAuthProvider } from '@convex-dev/auth/react'
import { RouterProvider } from '@tanstack/react-router'
import { ConvexReactClient } from 'convex/react'
import { createRoot } from 'react-dom/client'
import { router } from './App'
import './style.css'

const convexUrl = import.meta.env.VITE_CONVEX_URL
const convex = convexUrl ? new ConvexReactClient(convexUrl) : undefined

createRoot(document.getElementById('app')!).render(
  convex ? (
    <ConvexAuthProvider client={convex} replaceURL={url => router.history.replace(url)}>
      <RouterProvider router={router} />
    </ConvexAuthProvider>
  ) : <RouterProvider router={router} />,
)
