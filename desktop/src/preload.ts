import { contextBridge, ipcRenderer } from 'electron'

contextBridge.exposeInMainWorld('mqsDesktop', {
  platform: process.platform,
  setAutostart: (enabled: boolean) => ipcRenderer.invoke('mqs:setAutostart', enabled),
  getAutostart: () => ipcRenderer.invoke('mqs:getAutostart'),
})
