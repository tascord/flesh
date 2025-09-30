import { createRoot } from 'react-dom/client'
import { StrictMode } from 'react'
import { ChatProvider } from './api';
import Chat from './chat'
import './main.css';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <ChatProvider>
      <Chat />
    </ChatProvider>
  </StrictMode>,
)