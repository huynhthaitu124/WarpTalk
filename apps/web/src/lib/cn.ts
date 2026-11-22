import { clsx, type ClassValue } from 'clsx';
import { extendTailwindMerge } from 'tailwind-merge';

// Teach tailwind-merge our custom type-scale tokens so `text-body` (size) never clobbers
// `text-on-primary` (colour) and vice versa.
const twMerge = extendTailwindMerge({
  extend: {
    classGroups: {
      'font-size': [{ text: ['display-xl', 'display-lg', 'display-md', 'headline', 'settings', 'section', 'card-title', 'title', 'body-lg', 'body', 'body-sm', 'caption', 'eyebrow'] }],
    },
  },
});

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
