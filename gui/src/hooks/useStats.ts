import { useQuery } from '@tanstack/react-query';
import { fetchStats, type StatsResponse } from '../api/client';

export function useStats() {
  return useQuery<StatsResponse>({
    queryKey: ['stats'],
    queryFn: fetchStats,
    staleTime: 30_000,
  });
}
