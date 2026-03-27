import { useQuery } from '@tanstack/react-query';
import { useState, useEffect } from 'react';
import { searchResources, type Resource } from '../api/client';

/**
 * Debounce a string value by the given delay in milliseconds.
 */
function useDebouncedValue(value: string, delayMs: number): string {
  const [debounced, setDebounced] = useState(value);

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);

  return debounced;
}

export function useSearch(query: string, type?: string) {
  const debouncedQuery = useDebouncedValue(query, 300);

  return useQuery<Resource[]>({
    queryKey: ['search', debouncedQuery, type ?? 'all'],
    queryFn: () => searchResources(debouncedQuery, type),
    staleTime: 30_000,
  });
}
