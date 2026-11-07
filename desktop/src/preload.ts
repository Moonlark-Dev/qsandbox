import { contextBridge, ipcRenderer } from 'electron'

contextBridge.exposeInMainWorld('mqsDesktop', {
  platform: process.platform,
  setAutostart: (enabled: boolean) => ipcRenderer.invoke('mqs:setAutostart', enabled),
  getAutostart: () => ipcRenderer.invoke('mqs:getAutostart'),
  listProfiles: () => ipcRenderer.invoke('mqs:profiles'),
  switchProfile: (name: string) => ipcRenderer.invoke('mqs:switchProfile', name),
  profilesHome: () => ipcRenderer.invoke('mqs:profilesHome'),
})
