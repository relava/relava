import { useQuery } from '@tanstack/react-query';
import { fetchHealth, type HealthResponse } from '../api/client';

export function useHealth() {
  return useQuery<HealthResponse>({
    queryKey: ['health'],
    queryFn: fetchHealth,
    staleTime: 10_000,
  });
}
