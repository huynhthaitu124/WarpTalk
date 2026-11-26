import { create } from "zustand";

interface UIState {
  leftSidebarOpen: boolean;
  setLeftSidebarOpen: (open: boolean) => void;
  toggleLeftSidebar: () => void;
  rightSidebarOpen: boolean;
  setRightSidebarOpen: (open: boolean) => void;
  toggleRightSidebar: () => void;
  createRoomModalOpen: boolean;
  setCreateRoomModalOpen: (open: boolean) => void;
  searchMeetingModalOpen: boolean;
  setSearchMeetingModalOpen: (open: boolean) => void;
  editRoomId: string | null;
  setEditRoomId: (id: string | null) => void;
}

export const useUIStore = create<UIState>((set) => ({
  leftSidebarOpen: true,
  setLeftSidebarOpen: (open) => set({ leftSidebarOpen: open }),
  toggleLeftSidebar: () => set((state) => ({ leftSidebarOpen: !state.leftSidebarOpen })),
  rightSidebarOpen: false,
  setRightSidebarOpen: (open) => set({ rightSidebarOpen: open }),
  toggleRightSidebar: () => set((state) => ({ rightSidebarOpen: !state.rightSidebarOpen })),
  createRoomModalOpen: false,
  setCreateRoomModalOpen: (open) => set({ createRoomModalOpen: open }),
  searchMeetingModalOpen: false,
  setSearchMeetingModalOpen: (open) => set({ searchMeetingModalOpen: open }),
  editRoomId: null,
  setEditRoomId: (id) => set({ editRoomId: id }),
}));
